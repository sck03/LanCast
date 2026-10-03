//! A single explicitly opened resource, never a directory or arbitrary URL proxy.
use anyhow::ensure;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, Semaphore},
};

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
    active: AtomicBool,
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
            active: AtomicBool::new(true),
        }))
    }
    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }
    pub fn path(&self) -> String {
        format!("/media/{}", self.token)
    }
    pub async fn serve(
        self: Arc<Self>,
        listener: TcpListener,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> anyhow::Result<()> {
        let limit = Arc::new(Semaphore::new(4));
        while self.active.load(Ordering::Acquire) {
            let accepted = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;
            let Ok(Ok((stream, remote))) = accepted else {
                continue;
            };
            if remote.ip() != self.allowed {
                continue;
            }
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                continue;
            };
            let resource = self.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tokio::time::timeout(Duration::from_secs(120), async {
                    if let Some(config) = tls {
                        let stream = tokio_rustls::TlsAcceptor::from(config)
                            .accept(stream)
                            .await?;
                        resource.respond(stream).await
                    } else {
                        resource.respond(stream).await
                    }
                })
                .await;
            });
        }
        Ok(())
    }
    async fn respond<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut stream: S,
    ) -> anyhow::Result<()> {
        let mut header = Vec::new();
        loop {
            ensure!(header.len() < 8192, "HEADER_TOO_LARGE");
            let b = tokio::time::timeout(Duration::from_secs(5), stream.read_u8()).await??;
            header.push(b);
            if header.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let header = std::str::from_utf8(&header)?;
        let mut lines = header.split("\r\n");
        let mut first = lines.next().unwrap_or("").split_whitespace();
        let method = first.next().unwrap_or("");
        let path = first.next().unwrap_or("");
        if !self.active.load(Ordering::Acquire) || !crate::auth::same_secret(path, &self.path()) {
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        }
        if method != "GET" && method != "HEAD" {
            stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nAllow: GET, HEAD\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
            return Ok(());
        }
        let mut range = None;
        for line in lines {
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("range")
            {
                ensure!(range.is_none(), "DUPLICATE_RANGE");
                range = Some(value.trim());
            }
        }
        // RFC 9110: Range modifies GET; HEAD describes the complete representation.
        let requested = if method == "GET" {
            parse_range(range, self.length)
        } else {
            Range::Full
        };
        let (status, start, count, extra) = match requested {
            Range::Full => ("200 OK", 0, self.length, String::new()),
            Range::Partial { start, end } => (
                "206 Partial Content",
                start,
                end - start + 1,
                format!("Content-Range: bytes {start}-{end}/{}\r\n", self.length),
            ),
            Range::Unsatisfiable => (
                "416 Range Not Satisfiable",
                0,
                0,
                format!("Content-Range: bytes */{}\r\n", self.length),
            ),
        };
        stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: video/mp4\r\nContent-Length: {count}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n{extra}\r\n").as_bytes()).await?;
        if method == "HEAD" || count == 0 {
            return Ok(());
        }
        let mut file = self.file.lock().await;
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut left = count;
        let mut buffer = [0; 65536];
        while left > 0 && self.active.load(Ordering::Acquire) {
            let size = buffer.len().min(left as usize);
            let read = file.read(&mut buffer[..size]).await?;
            if read == 0 {
                break;
            }
            stream.write_all(&buffer[..read]).await?;
            left -= read as u64;
        }
        Ok(())
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
        let _ = resource.serve(listener, tls).await;
    });
    Ok((url, task))
}
#[cfg(test)]
mod tests {
    use super::*;
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
    async fn serves_only_grant_and_revokes() {
        use std::io::Write;
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"0123456789").unwrap();
        let resource = Resource::from_file(file, "127.0.0.1".parse().unwrap()).unwrap();
        let (url, task) = bind_resource(resource.clone(), "127.0.0.1:0".parse().unwrap(), None)
            .await
            .unwrap();
        let client = reqwest::Client::new();
        let r = client
            .get(&url)
            .header("Range", "bytes=2-4")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.text().await.unwrap(), "234");
        assert_eq!(
            client
                .get(format!("{url}/../secret"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        resource.revoke();
        assert_eq!(client.get(&url).send().await.unwrap().status(), 404);
        task.abort();
    }
}
