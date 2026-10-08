//! Real TCP tests for the production receiver; synthetic peers are not device certification.
use aes::cipher::{KeyIvInit, StreamCipher};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use lancast_airplay::engine::{Event, Output, Running};
use sha2::{Digest, Sha512};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpStream},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use x25519_dalek::{EphemeralSecret, PublicKey};

struct Capture(mpsc::Sender<Event>);
impl Output for Capture {
    fn emit(&self, event: Event) -> bool {
        self.0.send(event).is_ok()
    }
}

fn start() -> (Running, mpsc::Receiver<Event>, u16) {
    let (tx, rx) = mpsc::channel();
    let engine = Running::start(
        Ipv4Addr::LOCALHOST,
        "Loopback".into(),
        [1; 32],
        [2, 6, 4, 8, 2, 6, 4, 8],
        None,
        Arc::new(Capture(tx)),
    )
    .unwrap();
    let Event::Ready { port, .. } = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("receiver failed to start")
    };
    (engine, rx, port)
}
fn socket(port: u16) -> TcpStream {
    let s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
    s
}
fn send(s: &mut TcpStream, method: &str, path: &str, body: &[u8], kind: &str) {
    write!(
        s,
        "{method} {path} RTSP/1.0\r\nCSeq: 1\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(body).unwrap();
}
fn response(s: &mut TcpStream) -> (u16, Vec<u8>) {
    let mut header = Vec::new();
    let mut byte = [0];
    while !header.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() < 16384);
    }
    let header = String::from_utf8(header).unwrap();
    let status = header.split_whitespace().nth(1).unwrap().parse().unwrap();
    let len = header
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0; len];
    s.read_exact(&mut body).unwrap();
    (status, body)
}
fn exchange(
    s: &mut TcpStream,
    method: &str,
    path: &str,
    body: &[u8],
    kind: &str,
) -> (u16, Vec<u8>) {
    send(s, method, path, body, kind);
    response(s)
}
fn derive16(prefix: &[u8], secret: &[u8]) -> [u8; 16] {
    let mut h = Sha512::new();
    h.update(prefix);
    h.update(secret);
    h.finalize()[..16].try_into().unwrap()
}

/// Returns the verified shared secret. Checking the receiver signature proves that its
/// advertised public identity is not accidentally being reused as a private-key seed.
fn legacy_pair(s: &mut TcpStream, tamper: bool) -> u16 {
    let ephemeral = EphemeralSecret::random_from_rng(&mut rand10::rng());
    let public = PublicKey::from(&ephemeral);
    let signing = SigningKey::from_bytes(&[3; 32]);
    let (status, key) = exchange(
        s,
        "POST",
        "/pair-setup",
        signing.verifying_key().as_bytes(),
        "application/octet-stream",
    );
    assert_eq!(status, 200);
    assert_eq!(
        key,
        SigningKey::from_bytes(&[1; 32]).verifying_key().to_bytes()
    );
    let mut body = vec![1, 0, 0, 0];
    body.extend_from_slice(public.as_bytes());
    body.extend_from_slice(signing.verifying_key().as_bytes());
    let (status, reply) = exchange(s, "POST", "/pair-verify", &body, "application/octet-stream");
    assert_eq!(status, 200);
    assert_eq!(reply.len(), 96);
    let receiver = PublicKey::from(<[u8; 32]>::try_from(&reply[..32]).unwrap());
    let secret = ephemeral.diffie_hellman(&receiver);
    let aes = derive16(b"Pair-Verify-AES-Key", secret.as_bytes());
    let iv = derive16(b"Pair-Verify-AES-IV", secret.as_bytes());
    let mut cipher = ctr::Ctr128BE::<aes::Aes128>::new(&aes.into(), &iv.into());
    let mut signature = reply[32..].to_vec();
    cipher.apply_keystream(&mut signature);
    let message = [receiver.as_bytes().as_slice(), public.as_bytes().as_slice()].concat();
    VerifyingKey::from_bytes(&key.try_into().unwrap())
        .unwrap()
        .verify_strict(&message, &Signature::from_slice(&signature).unwrap())
        .unwrap();
    let mut signature = signing
        .sign(&[public.as_bytes().as_slice(), receiver.as_bytes().as_slice()].concat())
        .to_bytes();
    if tamper {
        signature[0] ^= 1;
    }
    cipher.apply_keystream(&mut signature);
    let mut body = vec![0; 4];
    body.extend_from_slice(&signature);
    exchange(s, "POST", "/pair-verify", &body, "application/octet-stream").0
}

#[test]
fn real_receiver_serves_capabilities_and_closes_listener_and_stalled_clients() {
    let (engine, _rx, port) = start();
    let mut s = socket(port);
    let (status, body) = exchange(&mut s, "GET", "/info", b"", "application/octet-stream");
    assert_eq!(status, 200);
    let info: plist::Value = plist::from_bytes(&body).unwrap();
    let dict = info.as_dictionary().unwrap();
    assert_eq!(dict["name"].as_string(), Some("Loopback"));
    assert_eq!(dict["audioFormats"].as_array().unwrap().len(), 1);
    s.write_all(b"POST /pair-setup RTSP/1.0\r\nContent-Length: 1000\r\n\r\nx")
        .unwrap();
    let begin = Instant::now();
    engine.stop();
    assert!(begin.elapsed() < Duration::from_secs(4));
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err());
    let mut buf = [0];
    assert!(matches!(s.read(&mut buf), Ok(0) | Err(_)));
}
#[test]
fn legacy_pairing_checks_both_signatures_and_rejects_mode_switch() {
    let (engine, _rx, port) = start();
    let mut good = socket(port);
    assert_eq!(legacy_pair(&mut good, false), 200);
    let (status, _) = exchange(
        &mut good,
        "POST",
        "/pair-setup",
        &[6, 1, 1, 0, 1, 0],
        "application/pairing+tlv8",
    );
    assert_eq!(status, 403);
    let mut bad = socket(port);
    assert_eq!(legacy_pair(&mut bad, true), 401);
    engine.stop();
}
#[test]
fn media_setup_requires_tv_approval_and_shutdown_cancels_pending_approval() {
    let (engine, rx, port) = start();
    let mut s = socket(port);
    assert_eq!(legacy_pair(&mut s, false), 200);
    let value = plist::Value::Dictionary(
        [
            (
                "name".to_string(),
                plist::Value::String("Synthetic phone".into()),
            ),
            ("model".to_string(), plist::Value::String("Test".into())),
            ("deviceID".to_string(), plist::Value::String("test".into())),
            (
                "macAddress".to_string(),
                plist::Value::String("00:11:22:33:44:55".into()),
            ),
            (
                "timingProtocol".to_string(),
                plist::Value::String("NTP".into()),
            ),
            (
                "timingPort".to_string(),
                plist::Value::Integer(12345.into()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let mut body = Vec::new();
    plist::to_writer_binary(&mut body, &value).unwrap();
    send(
        &mut s,
        "SETUP",
        "/42",
        &body,
        "application/x-apple-binary-plist",
    );
    let Event::Request { session, .. } = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("missing host approval")
    };
    assert!(session > 0);
    let begin = Instant::now();
    engine.stop();
    assert!(begin.elapsed() < Duration::from_secs(4));
    let mut out = [0];
    assert!(matches!(s.read(&mut out), Ok(0) | Err(_)));
}
