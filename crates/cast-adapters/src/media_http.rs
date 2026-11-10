//! Bounded, revocable file resources. Never accepts paths from an HTTP request.
use crate::{
    http_range::Range,
    http_server::{self, Body, empty},
};
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
pub struct Resource {
    file: Mutex<FileCursor>,
    pub length: u64,
    path: String,
    allowed: IpAddr,
    stop: CancellationToken,
}
struct FileCursor {
    file: tokio::fs::File,
    position: Option<u64>,
}
impl Resource {
    pub fn from_file(mut file: std::fs::File, allowed: IpAddr) -> anyhow::Result<Arc<Self>> {
        use std::io::Seek;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "MEDIA_UNSUPPORTED");
        let position = file.stream_position()?;
        Ok(Arc::new(Self {
            file: Mutex::new(FileCursor {
                file: tokio::fs::File::from_std(file),
                position: Some(position),
            }),
            length: metadata.len(),
            path: format!("/media/{}", crate::auth::random_url_token()),
            allowed,
            stop: CancellationToken::new(),
        }))
    }
    pub fn revoke(&self) {
        self.stop.cancel();
    }
    async fn respond(self: Arc<Self>, request: Request<Incoming>) -> Response<Body> {
        if self.stop.is_cancelled()
            || request.uri().query().is_some()
            || !crate::auth::same_secret(request.uri().path(), &self.path)
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
        // This grant has no ETag or Last-Modified validator. An If-Range condition
        // cannot match, so returning partial content could splice different files.
        let range = if head || request.headers().contains_key(header::IF_RANGE) {
            Range::Full
        } else {
            Range::parse(
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
                    let mut cursor = resource.file.lock().await;
                    if cursor.position != Some(offset) {
                        cursor.position = None;
                        cursor.file.seek(std::io::SeekFrom::Start(offset)).await?;
                    }
                    // Cancellation may drop this read after the OS has advanced
                    // its cursor. Invalidate the hint until completion is observed.
                    cursor.position = None;
                    let read = cursor.file.read(&mut buffer).await?;
                    cursor.position = Some(offset + read as u64);
                    read
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
        resource.path
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
    #[tokio::test]
    async fn unverifiable_if_range_returns_the_complete_representation() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"0123456789").unwrap();
        let resource = Resource::from_file(file, "127.0.0.1".parse().unwrap()).unwrap();
        let (url, task) = bind_resource(resource.clone(), "127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for validator in ["\"previous-file\"", "Wed, 21 Oct 2015 07:28:00 GMT"] {
            let response = client
                .get(&url)
                .header("Range", "bytes=2-4")
                .header("If-Range", validator)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert!(!response.headers().contains_key("content-range"));
            assert_eq!(response.headers()["content-length"], "10");
            assert_eq!(response.text().await.unwrap(), "0123456789");
        }
        resource.revoke();
        task.await.unwrap();
    }
    #[tokio::test]
    async fn range_head_concurrency_and_revocation() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"0123456789").unwrap();
        let resource = Resource::from_file(file, "127.0.0.1".parse().unwrap()).unwrap();
        let (url, task) = bind_resource(resource.clone(), "127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        for (range, status, content_range, body) in [
            (
                "bytes=2-18446744073709551616",
                206,
                Some("bytes 2-9/10"),
                "23456789",
            ),
            (
                "bytes=-18446744073709551616",
                206,
                Some("bytes 0-9/10"),
                "0123456789",
            ),
            ("BYTES=2-4", 206, Some("bytes 2-4/10"), "234"),
            ("bytes=+2-4", 416, Some("bytes */10"), ""),
            ("bytes=18446744073709551616-", 416, Some("bytes */10"), ""),
            ("bytes=0-1,4-5", 200, None, "0123456789"),
        ] {
            let response = c.get(&url).header("Range", range).send().await.unwrap();
            assert_eq!(response.status(), status, "{range}");
            assert_eq!(
                response
                    .headers()
                    .get("content-range")
                    .map(|value| value.to_str().unwrap()),
                content_range
            );
            assert_eq!(response.headers()["content-length"], body.len().to_string());
            assert_eq!(response.text().await.unwrap(), body);
        }
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
        // A disconnected request invalidates the shared cursor hint. The next
        // range still seeks explicitly and must not read from the old offset.
        {
            let mut cursor = resource.file.lock().await;
            cursor.file.seek(std::io::SeekFrom::Start(7)).await.unwrap();
            cursor.position = None;
        }
        assert_eq!(
            c.get(&url)
                .header("Range", "bytes=1-3")
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "123"
        );
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
