//! Hyper owns HTTP framing; all connections observe the same revocation token.
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{convert::Infallible, future::Future, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
pub type Body = UnsyncBoxBody<Bytes, std::io::Error>;
pub fn empty(status: u16) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(
            Full::new(Bytes::new())
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .unwrap()
}
pub async fn serve<F, Fut>(
    listener: TcpListener,
    allowed: std::net::IpAddr,
    tls: Option<Arc<rustls::ServerConfig>>,
    stop: CancellationToken,
    handler: F,
) -> anyhow::Result<()>
where
    F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response<Body>> + Send + 'static,
{
    let mut connections = JoinSet::new();
    let result = loop {
        let accepted = tokio::select! { biased;
            _ = stop.cancelled() => break Ok(()),
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            accepted = listener.accept() => accepted,
        };
        let (stream, address) = match accepted {
            Ok(connection) => connection,
            Err(error) => break Err(error.into()),
        };
        // Reap completed tasks before accepting another connection so they never
        // retain resources or occupy the four live connection slots.
        if address.ip() != allowed || connections.len() >= 4 {
            continue;
        }
        let handler = handler.clone();
        let stop = stop.clone();
        let tls = tls.clone();
        connections.spawn(async move {
            if let Some(config) = tls {
                let acceptor = tokio_rustls::TlsAcceptor::from(config);
                let handshake =
                    tokio::time::timeout(Duration::from_secs(5), acceptor.accept(stream));
                let result =
                    tokio::select! { biased; _ = stop.cancelled() => return, r = handshake => r };
                if let Ok(Ok(stream)) = result {
                    connection(stream, stop, handler).await;
                }
            } else {
                connection(stream, stop, handler).await;
            }
        });
    };
    // Covers revocation and accept errors. Dropping/aborting the listener also
    // aborts its owned tasks through JoinSet rather than detaching them.
    connections.shutdown().await;
    result
}
async fn connection<S, F, Fut>(stream: S, stop: CancellationToken, handler: F)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Fn(Request<Incoming>) -> Fut + Send + 'static,
    Fut: Future<Output = Response<Body>> + Send + 'static,
{
    let connection_stop = stop.child_token();
    let _cancel_on_exit = connection_stop.clone().drop_guard();
    let request_stop = connection_stop.clone();
    let service = service_fn(move |mut r: Request<Incoming>| {
        r.extensions_mut().insert(request_stop.clone());
        let f = handler(r);
        async move { Ok::<_, Infallible>(f.await) }
    });
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(5))
        .max_headers(32)
        .max_buf_size(16384)
        .keep_alive(false);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::select! { biased; _ = connection_stop.cancelled() => {}, _ = connection => {} }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        sync::{Semaphore, mpsc},
    };

    async fn start<F, Fut>(
        handler: F,
    ) -> (
        std::net::SocketAddr,
        CancellationToken,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    )
    where
        F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response<Body>> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let task = tokio::spawn(serve(listener, address.ip(), None, stop.clone(), handler));
        (address, stop, task)
    }

    #[tokio::test]
    async fn completed_connections_cancel_request_work_and_release_slots() {
        let (tx, mut rx) = mpsc::channel(1);
        let (address, stop, task) = start(move |request| {
            tx.try_send(
                request
                    .extensions()
                    .get::<CancellationToken>()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
            async { empty(200) }
        })
        .await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        for _ in 0..12 {
            assert_eq!(
                client
                    .get(format!("http://{address}/"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
            let request_stop = rx.recv().await.unwrap();
            if tokio::time::timeout(Duration::from_millis(200), request_stop.cancelled())
                .await
                .is_err()
            {
                stop.cancel();
                task.await.unwrap().unwrap();
                panic!("completed HTTP connection left request work alive");
            }
        }
        stop.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn connection_limit_recovers_after_a_request_finishes() {
        let (tx, mut rx) = mpsc::channel(4);
        let release = Arc::new(Semaphore::new(0));
        let gate = release.clone();
        let (address, stop, task) = start(move |request| {
            let gate = gate.clone();
            tx.try_send(
                request
                    .extensions()
                    .get::<CancellationToken>()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
            async move {
                gate.acquire().await.unwrap().forget();
                empty(200)
            }
        })
        .await;
        let mut clients = Vec::new();
        let mut tokens = Vec::new();
        for _ in 0..4 {
            let mut client = TcpStream::connect(address).await.unwrap();
            client
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            tokens.push(
                tokio::time::timeout(Duration::from_secs(2), rx.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
            clients.push(client);
        }
        let mut excess = TcpStream::connect(address).await.unwrap();
        let mut byte = [0];
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), excess.read(&mut byte))
                .await
                .unwrap(),
            Ok(0) | Err(_)
        ));
        assert!(tokens.iter().all(|token| !token.is_cancelled()));
        release.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), tokens[0].cancelled())
            .await
            .unwrap();
        let mut replacement = TcpStream::connect(address).await.unwrap();
        replacement
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let replacement_stop = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        stop.cancel();
        task.await.unwrap().unwrap();
        assert!(replacement_stop.is_cancelled());
        assert!(tokens.iter().all(CancellationToken::is_cancelled));
    }

    #[tokio::test]
    async fn stopping_or_aborting_listener_releases_pending_handlers() {
        for abort in [false, true] {
            let owner = Arc::new(());
            let lease = owner.clone();
            let (tx, mut rx) = mpsc::channel(1);
            let (address, stop, task) = start(move |request| {
                let lease = lease.clone();
                tx.try_send(
                    request
                        .extensions()
                        .get::<CancellationToken>()
                        .unwrap()
                        .clone(),
                )
                .unwrap();
                async move {
                    let _lease = lease;
                    std::future::pending::<Response<Body>>().await
                }
            })
            .await;
            let mut client = TcpStream::connect(address).await.unwrap();
            client
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
            let request_stop = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            if abort {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            } else {
                stop.cancel();
                task.await.unwrap().unwrap();
                assert_eq!(
                    Arc::strong_count(&owner),
                    1,
                    "stop returned before handler cleanup"
                );
            }
            let cancelled =
                tokio::time::timeout(Duration::from_millis(200), request_stop.cancelled()).await;
            stop.cancel();
            assert!(
                cancelled.is_ok(),
                "aborted listener left request work alive"
            );
            assert_eq!(Arc::strong_count(&owner), 1);
        }
    }
}
