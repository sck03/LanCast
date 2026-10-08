//! LanCast framing: bounded, incremental and pipeline-safe RTSP <-> HTTP adaptation.
use bytes::{Buf, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;

#[derive(Default)]
pub struct Rtsp2Http {
    response: BytesMut,
}

pub trait ResponseBoundary {
    fn at_boundary(&self) -> bool;
}
impl ResponseBoundary for Rtsp2Http {
    fn at_boundary(&self) -> bool {
        self.response.is_empty()
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn head_end(src: &[u8]) -> io::Result<Option<usize>> {
    let end = src.windows(4).position(|p| p == b"\r\n\r\n").map(|n| n + 4);
    if end.is_some_and(|n| n > MAX_HEAD) || (end.is_none() && src.len() > MAX_HEAD) {
        return Err(invalid("header limit"));
    }
    Ok(end)
}

fn body_length(headers: &[httparse::Header<'_>]) -> io::Result<usize> {
    let mut size = None;
    for h in headers {
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(invalid("chunked RTSP unsupported"));
        }
        if h.name.eq_ignore_ascii_case("content-length") {
            if size.is_some() {
                return Err(invalid("duplicate content-length"));
            }
            let value = std::str::from_utf8(h.value).map_err(|_| invalid("content-length"))?;
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("content-length"));
            }
            let n: usize = value.parse().map_err(|_| invalid("content-length"))?;
            if n > MAX_BODY {
                return Err(invalid("body limit"));
            }
            size = Some(n);
        }
    }
    Ok(size.unwrap_or(0))
}

impl Decoder for Rtsp2Http {
    type Item = BytesMut;
    type Error = io::Error;
    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<BytesMut>> {
        let Some(end) = head_end(src)? else {
            return Ok(None);
        };
        let mut head = src[..end].to_vec();
        let first_end = head
            .windows(2)
            .position(|p| p == b"\r\n")
            .ok_or_else(|| invalid("request line"))?;
        if first_end < 8 {
            return Err(invalid("request line"));
        }
        match &head[first_end - 8..first_end] {
            b"RTSP/1.0" => head[first_end - 8..first_end].copy_from_slice(b"HTTP/1.1"),
            b"HTTP/1.1" => {}
            _ => return Err(invalid("protocol version")),
        }
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers);
        req.parse(&head).map_err(|_| invalid("request headers"))?;
        let len = body_length(req.headers)?;
        if src.len() < end + len {
            return Ok(None);
        }
        let path = req.path.ok_or_else(|| invalid("path"))?;
        let path = if path == "*" {
            "/"
        } else if let Some(rest) = path.strip_prefix("rtsp://") {
            rest.find('/').map(|n| &rest[n..]).unwrap_or("/")
        } else {
            path
        };
        if !path.starts_with('/') {
            return Err(invalid("absolute path required"));
        }
        let mut output = BytesMut::new();
        output.extend_from_slice(req.method.ok_or_else(|| invalid("method"))?.as_bytes());
        output.extend_from_slice(b" ");
        output.extend_from_slice(path.as_bytes());
        output.extend_from_slice(b" HTTP/1.1\r\n");
        output.extend_from_slice(&head[first_end + 2..]);
        output.extend_from_slice(&src[end..end + len]);
        src.advance(end + len);
        Ok(Some(output))
    }
}

impl<T: AsRef<[u8]>> Encoder<T> for Rtsp2Http {
    type Error = io::Error;
    fn encode(&mut self, item: T, dst: &mut BytesMut) -> io::Result<()> {
        if self.response.len().saturating_add(item.as_ref().len()) > MAX_HEAD + MAX_BODY {
            return Err(invalid("response limit"));
        }
        self.response.extend_from_slice(item.as_ref());
        while let Some(end) = head_end(&self.response)? {
            let mut headers = [httparse::EMPTY_HEADER; 64];
            let mut res = httparse::Response::new(&mut headers);
            res.parse(&self.response[..end])
                .map_err(|_| invalid("response headers"))?;
            let len = body_length(res.headers)?;
            if self.response.len() < end + len {
                break;
            }
            let mut frame = self.response.split_to(end + len);
            frame[..8].copy_from_slice(b"RTSP/1.0");
            dst.extend_from_slice(&frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_and_pipelined_messages_preserve_body_and_next_request() {
        let request =
            b"SETUP rtsp://[fe80::1]/42 RTSP/1.0\r\nContent-Length: 3\r\nCSeq: 1\r\n\r\nabc";
        let mut codec = Rtsp2Http::default();
        let mut input = BytesMut::new();
        for b in &request[..request.len() - 1] {
            input.extend_from_slice(&[*b]);
            assert!(codec.decode(&mut input).unwrap().is_none());
        }
        input.extend_from_slice(&request[request.len() - 1..]);
        input.extend_from_slice(b"OPTIONS * RTSP/1.0\r\nCSeq: 2\r\n\r\n");
        let first = codec.decode(&mut input).unwrap().unwrap();
        assert!(first.starts_with(b"SETUP /42 HTTP/1.1"));
        assert!(first.ends_with(b"abc"));
        assert!(
            codec
                .decode(&mut input)
                .unwrap()
                .unwrap()
                .starts_with(b"OPTIONS / HTTP/1.1")
        );
        assert!(input.is_empty());
    }
    #[test]
    fn response_can_be_split_between_header_and_body() {
        let mut codec = Rtsp2Http::default();
        let mut dst = BytesMut::new();
        codec
            .encode(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\na", &mut dst)
            .unwrap();
        assert!(dst.is_empty());
        assert!(!codec.at_boundary());
        codec.encode(b"bc", &mut dst).unwrap();
        assert_eq!(
            dst.as_ref(),
            b"RTSP/1.0 200 OK\r\nContent-Length: 3\r\n\r\nabc"
        );
        assert!(codec.at_boundary());
    }
    #[test]
    fn lengths_and_ambiguous_framing_are_rejected_before_allocation() {
        for headers in [
            "Content-Length: 999999999",
            "Content-Length: -1",
            "Content-Length: 1\r\nContent-Length: 1",
            "Transfer-Encoding: chunked",
        ] {
            let mut frame =
                BytesMut::from(format!("POST / RTSP/1.0\r\n{headers}\r\n\r\n").as_bytes());
            assert!(Rtsp2Http::default().decode(&mut frame).is_err());
        }
    }
}
