#![cfg(feature = "legacy")]
use cast_adapters::{
    auth::Identity,
    legacy_bridge::Bridge,
    media_http::{Resource, bind_resource},
};
use std::{io::Write, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn upstream() -> (
    String,
    Identity,
    tokio::task::JoinHandle<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>,
) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let identity = Identity::create().unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(identity.server_config().unwrap()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}/media/test", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(socket).await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.unwrap());
            assert!(request.len() < 4096);
        }
        stream
    });
    (url, identity, task)
}

#[tokio::test]
async fn slow_upstream_headers_cannot_extend_deadline() {
    let (remote, identity, connection) = upstream().await;
    let (local, bridge, task) = Bridge::bind(&remote, &identity.fingerprint).await.unwrap();
    let upstream = tokio::spawn(async move {
        let mut stream = connection.await.unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Progress: ")
            .await
            .unwrap();
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if stream.write_all(b"a").await.is_err() {
                break;
            }
        }
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = tokio::time::timeout(Duration::from_secs(7), client.get(local).send())
        .await
        .expect("trickled headers must not retain a bridge connection indefinitely")
        .unwrap();
    assert_eq!(response.status(), 502);
    bridge.revoke();
    task.await.unwrap();
    upstream.abort();
    let _ = upstream.await;
}

#[tokio::test]
async fn header_deadline_does_not_limit_active_media_body() {
    let (remote, identity, connection) = upstream().await;
    let (local, bridge, task) = Bridge::bind(&remote, &identity.fingerprint).await.unwrap();
    let upstream = tokio::spawn(async move {
        let mut stream = connection.await.unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        for byte in b"012345" {
            tokio::time::sleep(Duration::from_secs(1)).await;
            stream.write_all(&[*byte]).await.unwrap();
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let response = client.get(local).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "012345");
    bridge.revoke();
    task.await.unwrap();
    upstream.await.unwrap();
}

#[tokio::test]
async fn loopback_preserves_range_pinning_and_revocation() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let identity = Identity::create().unwrap();
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(b"0123456789").unwrap();
    let resource = Resource::from_file(file, "127.0.0.1".parse().unwrap()).unwrap();
    let (remote, remote_task) = bind_resource(
        resource.clone(),
        "127.0.0.1:0".parse().unwrap(),
        Some(Arc::new(identity.server_config().unwrap())),
    )
    .await
    .unwrap();
    let (local, bridge, task) = Bridge::bind(&remote, &identity.fingerprint).await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .get(&local)
        .header("Range", "bytes=3-5")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 3-5/10");
    assert_eq!(response.text().await.unwrap(), "345");
    let head = client.head(&local).send().await.unwrap();
    assert_eq!(head.headers()["content-length"], "10");
    let (wrong, wrong_bridge, wrong_task) = Bridge::bind(&remote, &"00".repeat(32)).await.unwrap();
    assert_eq!(client.get(&wrong).send().await.unwrap().status(), 502);
    bridge.revoke();
    wrong_bridge.revoke();
    resource.revoke();
    task.await.unwrap();
    wrong_task.await.unwrap();
    remote_task.await.unwrap();
    assert!(client.get(local).send().await.is_err());
}
