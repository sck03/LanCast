#![cfg(feature = "legacy")]
use cast_adapters::{
    auth::Identity,
    legacy_bridge::Bridge,
    media_http::{Resource, bind_resource},
};
use std::{io::Write, sync::Arc};

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
