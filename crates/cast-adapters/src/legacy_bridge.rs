//! One pinned HTTPS grant exposed only on loopback for Android's system MediaPlayer.
use crate::{
    auth,
    http_server::{self, Body, empty},
};
use anyhow::{Context, ensure};
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    Request, Response,
    body::{Frame, Incoming},
    header,
};
use std::{net::IpAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
pub struct Bridge {
    client: reqwest::Client,
    upstream: String,
    path: String,
    stop: CancellationToken,
}
impl Bridge {
    pub async fn bind(
        upstream: &str,
        fingerprint: &str,
    ) -> anyhow::Result<(String, Arc<Self>, tokio::task::JoinHandle<()>)> {
        let url = url::Url::parse(upstream)?;
        let ip: IpAddr = url
            .host_str()
            .context("NO_HOST")?
            .trim_matches(['[', ']'])
            .parse()?;
        ensure!(
            url.scheme() == "https"
                && (crate::dlna::lan_ip(ip) || ip.is_loopback())
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path().starts_with("/media/"),
            "UNSAFE_MEDIA_URL"
        );
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(5))
            .use_preconfigured_tls(auth::pinned_config(fingerprint)?)
            .build()?;
        let bridge = Arc::new(Self {
            client,
            upstream: url.to_string(),
            path: format!(
                "/bridge/{}",
                auth::random_token().replace('/', "_").replace('+', "-")
            ),
            stop: CancellationToken::new(),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let local = format!("http://{}{}", listener.local_addr()?, bridge.path);
        let server = bridge.clone();
        let task = tokio::spawn(async move {
            let stop = server.stop.clone();
            let _ = http_server::serve(
                listener,
                "127.0.0.1".parse().unwrap(),
                None,
                stop,
                move |r| server.clone().respond(r),
            )
            .await;
        });
        Ok((local, bridge, task))
    }
    pub fn revoke(&self) {
        self.stop.cancel();
    }
    async fn respond(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if self.stop.is_cancelled()
            || request.uri().query().is_some()
            || !auth::same_secret(request.uri().path(), &self.path)
        {
            return empty(404);
        }
        if !matches!(*request.method(), hyper::Method::GET | hyper::Method::HEAD) {
            return empty(405);
        }
        if request.headers().get_all(header::RANGE).iter().count() > 1 {
            return empty(400);
        }
        let mut upstream = self
            .client
            .request(request.method().clone(), &self.upstream);
        if let Some(range) = request.headers().get(header::RANGE) {
            upstream = upstream.header(header::RANGE, range);
        }
        let result = tokio::select! { biased; _ = self.stop.cancelled() => return empty(410), r = upstream.send() => r };
        let Ok(upstream) = result else {
            return empty(502);
        };
        if !matches!(upstream.status().as_u16(), 200 | 206 | 416) {
            return empty(502);
        }
        let mut response = empty(upstream.status().as_u16());
        for name in [
            header::CONTENT_LENGTH,
            header::CONTENT_RANGE,
            header::ACCEPT_RANGES,
            header::CONTENT_TYPE,
        ] {
            if let Some(value) = upstream.headers().get(&name) {
                response.headers_mut().insert(name, value.clone());
            }
        }
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        if request.method() == hyper::Method::HEAD {
            return response;
        }
        let frames = upstream
            .bytes_stream()
            .map_err(std::io::Error::other)
            .and_then(|bytes| async {
                if bytes.len() > 256 * 1024 {
                    return Err(std::io::Error::other("UPSTREAM_CHUNK_TOO_LARGE"));
                }
                Ok(Frame::data(bytes))
            });
        *response.body_mut() = StreamBody::new(frames).boxed_unsync();
        response
    }
}
