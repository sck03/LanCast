//! Hyper owns HTTP framing; all connections observe the same revocation token.
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{convert::Infallible, future::Future, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::Semaphore,
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
    let slots = Arc::new(Semaphore::new(4));
    loop {
        let (stream, address) = tokio::select! { biased; _ = stop.cancelled() => break, accepted = listener.accept() => accepted? };
        if address.ip() != allowed {
            continue;
        }
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let handler = handler.clone();
        let stop = stop.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _permit = permit;
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
    }
    Ok(())
}
async fn connection<S, F, Fut>(stream: S, stop: CancellationToken, handler: F)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Fn(Request<Incoming>) -> Fut + Send + 'static,
    Fut: Future<Output = Response<Body>> + Send + 'static,
{
    let service = service_fn(move |r| {
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
    tokio::select! { biased; _ = stop.cancelled() => {}, _ = connection => {} }
}
