//! Bounded, revocable file resources. Never accepts paths from an HTTP request.
use crate::http_server::{self, Body, empty};
use anyhow::ensure;
use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    Request, Response,
    body::{Frame, Incoming},
    header,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    net::TcpListener,
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
#[derive(Debug, PartialEq)]
pub enum Range {
    Full,
    Partial { start: u64, end: u64 },
    Unsatisfiable,
}
pub fn parse_range(header: Option<&str>, length: u64) -> Range {
    let Some(raw) = header else {
        return Range::Full;
    };
    let Some(raw) = raw.strip_prefix("bytes=") else {
        return Range::Full;
    };
    if raw.contains(',') {
        return Range::Full;
    }
    let Some((left, right)) = raw.split_once('-') else {
        return Range::Unsatisfiable;
    };
    if length == 0 {
        return Range::Unsatisfiable;
    }
    if left.is_empty() {
        return match right.parse::<u64>() {
            Ok(n) if n > 0 => Range::Partial {
                start: length.saturating_sub(n),
                end: length - 1,
            },
            _ => Range::Unsatisfiable,
        };
    }
    let Ok(start) = left.parse::<u64>() else {
        return Range::Unsatisfiable;
    };
    let end = if right.is_empty() {
        length - 1
    } else {
        match right.parse::<u64>() {
            Ok(n) => n.min(length - 1),
            _ => return Range::Unsatisfiable,
        }
    };
    if start >= length || start > end {
        Range::Unsatisfiable
    } else {
        Range::Partial { start, end }
    }
}

pub struct Resource {
    file: Mutex<tokio::fs::File>,
    pub length: u64,
    token: String,
    allowed: IpAddr,
    stop: CancellationToken,
}
impl Resource {
    pub fn from_file(file: std::fs::File, allowed: IpAddr) -> anyhow::Result<Arc<Self>> {
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "MEDIA_UNSUPPORTED");
        Ok(Arc::new(Self {
            file: Mutex::new(tokio::fs::File::from_std(file)),
            length: metadata.len(),
            token: crate::auth::random_token()
                .replace('/', "_")
                .replace('+', "-"),
            allowed,
            stop: CancellationToken::new(),
        }))
    }
    pub fn revoke(&self) {
        self.stop.cancel();
    }
    pub fn path(&self) -> String {
        format!("/media/{}", self.token)
    }
    async fn respond(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if self.stop.is_cancelled()
            || request.uri().query().is_some()
            || !crate::auth::same_secret(request.uri().path(), &self.path())
        {
            return empty(404);
        }
        let head = request.method() == hyper::Method::HEAD;
        if request.method() != hyper::Method::GET && !head {
            let mut r = empty(405);
            r.headers_mut()
                .insert(header::ALLOW, "GET, HEAD".parse().unwrap());
            return r;
        }
        if request.headers().get_all(header::RANGE).iter().count() > 1 {
            return empty(400);
        }
        let range = if head {
            Range::Full
        } else {
            parse_range(
                request
                    .headers()
                    .get(header::RANGE)
                    .and_then(|h| h.to_str().ok()),
                self.length,
            )
        };
        let (status, start, count, content_range) = match range {
            Range::Full => (200, 0, self.length, None),
            Range::Partial { start, end } => (
                206,
                start,
                end - start + 1,
                Some(format!("bytes {start}-{end}/{}", self.length)),
            ),
            Range::Unsatisfiable => (416, 0, 0, Some(format!("bytes */{}", self.length))),
        };
        let mut response = empty(status);
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, "video/mp4".parse().unwrap());
        headers.insert(header::CONTENT_LENGTH, count.to_string().parse().unwrap());
        headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
        headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        if let Some(range) = content_range {
            headers.insert(header::CONTENT_RANGE, range.parse().unwrap());
        }
        if head || count == 0 {
            return response;
        }
        // Lock protects seek + read only; NEVER hold it while awaiting a client socket.
        let chunks = stream::try_unfold(
            (self, start, count),
            |(resource, offset, left)| async move {
                if left == 0 {
                    return Ok(None);
                }
                if resource.stop.is_cancelled() {
                    return Err(std::io::Error::other("RESOURCE_REVOKED"));
                }
                let mut buffer = vec![0; left.min(65536) as usize];
                let read = {
                    let mut file = resource.file.lock().await;
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                    file.read(&mut buffer).await?
                };
                if read == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "FILE_CHANGED",
                    ));
                }
                buffer.truncate(read);
                Ok(Some((
                    Frame::data(Bytes::from(buffer)),
                    (resource, offset + read as u64, left - read as u64),
                )))
            },
        );
        *response.body_mut() = StreamBody::new(chunks).boxed_unsync();
        response
    }
}
pub async fn bind_resource(
    resource: Arc<Resource>,
    address: SocketAddr,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> anyhow::Result<(String, tokio::task::JoinHandle<()>)> {
    ensure!(!address.ip().is_unspecified(), "SELECT_LAN_INTERFACE");
    let listener = TcpListener::bind(address).await?;
    let url = format!(
        "{}://{}{}",
        if tls.is_some() { "https" } else { "http" },
        listener.local_addr()?,
        resource.path()
    );
    let task = tokio::spawn(async move {
        let handler = resource.clone();
        let _ = http_server::serve(
            listener,
            resource.allowed,
            tls,
            resource.stop.clone(),
            move |r| handler.clone().respond(r),
        )
        .await;
    });
    Ok((url, task))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn range_edges() {
        assert_eq!(
            parse_range(Some("bytes=-4"), 10),
            Range::Partial { start: 6, end: 9 }
        );
        assert_eq!(
            parse_range(Some("bytes=8-99"), 10),
            Range::Partial { start: 8, end: 9 }
        );
        for h in [
            "bytes=10-",
            "bytes=-0",
            "bytes=5-4",
            "bytes=999999999999999999999999-",
        ] {
            assert_eq!(parse_range(Some(h), 10), Range::Unsatisfiable);
        }
        assert_eq!(parse_range(Some("bytes=0-"), 0), Range::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,4-5"), 10), Range::Full);
    }
    #[tokio::test]
    async fn range_head_concurrency_and_revocation() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"0123456789").unwrap();
        let resource = Resource::from_file(file, "127.0.0.1".parse().unwrap()).unwrap();
        let (url, task) = bind_resource(resource.clone(), "127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        let (a, b) = tokio::join!(
            c.get(&url).header("Range", "bytes=2-4").send(),
            c.get(&url).header("Range", "bytes=-2").send()
        );
        let a = a.unwrap();
        assert_eq!(a.status(), 206);
        assert_eq!(a.text().await.unwrap(), "234");
        assert_eq!(b.unwrap().text().await.unwrap(), "89");
        let h = c
            .head(&url)
            .header("Range", "bytes=2-4")
            .send()
            .await
            .unwrap();
        assert_eq!(h.status(), 200);
        assert_eq!(h.headers()["content-length"], "10");
        assert!(h.bytes().await.unwrap().is_empty());
        let r = c
            .get(&url)
            .header("Range", "bytes=10-")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 416);
        assert_eq!(r.headers()["content-range"], "bytes */10");
        assert_eq!(
            c.get(format!("{url}/secret"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        resource.revoke();
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(c.get(&url).send().await.is_err());
    }
}
