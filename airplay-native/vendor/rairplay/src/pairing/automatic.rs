//! LanCast: select a pairing protocol exactly once per TCP connection.
//! Authentication failure never retries another protocol or weakens access control.
use super::SharedSessionKey;
use crate::config::{Keychain, PinCode};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    response::IntoResponse,
    routing::post,
};
use http::StatusCode;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use yoke::{Yoke, erased::ErasedArcCart};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Modern,
    Legacy,
}

fn classify(body: &[u8], content_type: &str) -> Option<Mode> {
    if content_type.starts_with("application/pairing+tlv8")
        || body.starts_with(&[6, 1])
        || body.starts_with(&[0, 1, 0])
    {
        Some(Mode::Modern)
    } else if body.len() == 32 || (body.len() == 68 && body[1..4] == [0, 0, 0]) {
        Some(Mode::Legacy)
    } else {
        None
    }
}

pub fn router<K: Keychain>(
    keychain: Yoke<&'static K, ErasedArcCart>,
    key: SharedSessionKey,
    pin: Option<PinCode>,
) -> Router {
    let modern = super::homekit::router(keychain.clone(), key.clone(), pin);
    let legacy = super::legacy::router(keychain, key.clone());
    let mode = Arc::new(Mutex::new(None));
    let handler = move |req: Request| {
        let (modern, legacy, mode, key) =
            (modern.clone(), legacy.clone(), mode.clone(), key.clone());
        async move {
            if key.read().is_some() {
                return (StatusCode::FORBIDDEN, [("Connection", "close")]).into_response();
            }
            let (parts, body) = req.into_parts();
            let Ok(body) = to_bytes(body, 16 * 1024).await else {
                return StatusCode::PAYLOAD_TOO_LARGE.into_response();
            };
            let Some(requested) = classify(
                &body,
                parts
                    .headers
                    .get("content-type")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or(""),
            ) else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            {
                let mut selected = mode.lock().unwrap();
                if selected.is_some_and(|m| m != requested) {
                    return StatusCode::FORBIDDEN.into_response();
                }
                *selected = Some(requested);
            }
            let target = if requested == Mode::Modern {
                modern
            } else {
                legacy
            };
            let response = target
                .oneshot(Request::from_parts(parts, Body::from(body)))
                .await
                .unwrap();
            let (mut parts, body) = response.into_parts();
            let Ok(bytes) = to_bytes(body, 16 * 1024).await else {
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            };
            let mut failed = parts.status.is_client_error() || parts.status.is_server_error();
            if requested == Mode::Modern {
                let mut pos = 0;
                while pos + 2 <= bytes.len() {
                    let tag = bytes[pos];
                    let len = usize::from(bytes[pos + 1]);
                    pos += 2;
                    if pos + len > bytes.len() {
                        failed = true;
                        break;
                    }
                    if tag == 7 {
                        failed = true;
                    }
                    pos += len;
                }
            }
            if failed {
                parts.headers.insert(
                    http::header::CONNECTION,
                    http::HeaderValue::from_static("close"),
                );
            }
            axum::response::Response::from_parts(parts, Body::from(bytes))
        }
    };
    Router::new()
        .route("/pair-setup", post(handler.clone()))
        .route("/pair-verify", post(handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguish_wire_formats_without_guessing_errors() {
        assert!(classify(&[6, 1, 1], "").is_some_and(|m| m == Mode::Modern));
        assert!(classify(&[0; 68], "").is_some_and(|m| m == Mode::Legacy));
        assert!(classify(&[0; 5], "").is_none());
    }
}
