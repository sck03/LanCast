//! LAN HTTP policy. Trust is explicit and independent of the host platform verifier.
use std::time::Duration;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn builder(tls: rustls::ClientConfig) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .http1_only()
        .pool_max_idle_per_host(1)
        .connect_timeout(TIMEOUT)
        .read_timeout(TIMEOUT)
        .tls_backend_preconfigured(tls)
}

pub(crate) fn public_roots() -> rustls::ClientConfig {
    rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn reply(raw: String) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
                assert!(request.len() < 4096);
            }
            socket.write_all(raw.as_bytes()).await.unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn responses_enforce_header_budget_without_following_redirects() {
        let client = crate::dlna::client().unwrap();
        // Keep Hyper's default 100-header stack buffer; overriding its size
        // would allocate a header buffer on the heap for every response.
        for headers in [100, 101] {
            let raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n{}\r\n",
                "X-Test: value\r\n".repeat(headers - 1)
            );
            let (url, task) = reply(raw).await;
            let response = client.get(url).send().await;
            assert_eq!(response.is_ok(), headers == 100);
            task.await.unwrap();
        }
        let (url, task) = reply(
            "HTTP/1.1 302 Found\r\nContent-Length: 0\r\nLocation: http://127.0.0.1:1/\r\n\r\n"
                .into(),
        )
        .await;
        assert_eq!(client.get(url).send().await.unwrap().status(), 302);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn public_trust_does_not_accept_a_self_signed_media_identity() {
        let identity = crate::auth::Identity::create().unwrap();
        let acceptor =
            tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(identity.server_config().unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            assert!(acceptor.accept(socket).await.is_err());
        });
        assert!(
            crate::dlna::client()
                .unwrap()
                .get(url)
                .send()
                .await
                .is_err()
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_requests_retain_only_one_idle_socket_per_host() {
        use std::sync::Arc;
        use tokio::sync::{Barrier, mpsc};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let ready = Arc::new(Barrier::new(3));
        let (closed, mut closures) = mpsc::channel(3);
        let server = tokio::spawn(async move {
            let mut workers = tokio::task::JoinSet::new();
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let ready = ready.clone();
                let closed = closed.clone();
                workers.spawn(async move {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                        assert!(request.len() < 4096);
                    }
                    // All requests are active before any connection becomes idle.
                    ready.wait().await;
                    socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                    assert_eq!(socket.read(&mut [0]).await.unwrap(), 0);
                    closed.send(()).await.unwrap();
                });
            }
            while let Some(result) = workers.join_next().await {
                result.unwrap();
            }
        });
        let client = crate::dlna::client().unwrap();
        let requests = (0..3).map(|_| client.get(&url).send());
        for response in futures_util::future::join_all(requests).await {
            response.unwrap().bytes().await.unwrap();
        }
        for _ in 0..2 {
            tokio::time::timeout(std::time::Duration::from_secs(2), closures.recv())
                .await
                .expect("excess idle sockets must close promptly")
                .unwrap();
        }
        assert!(closures.try_recv().is_err());
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
}
