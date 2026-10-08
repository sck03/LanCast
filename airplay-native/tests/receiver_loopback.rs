//! Real TCP tests for the production receiver; synthetic peers are not device certification.
use aes::cipher::{KeyIvInit, StreamCipher};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce, aead::AeadInOut};
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

fn tlv(fields: &[(u8, &[u8])]) -> Vec<u8> {
    let mut result = Vec::new();
    for (tag, value) in fields {
        for chunk in value.chunks(255) {
            result.extend_from_slice(&[*tag, chunk.len() as u8]);
            result.extend_from_slice(chunk);
        }
    }
    result
}
fn tags(bytes: &[u8]) -> std::collections::BTreeMap<u8, Vec<u8>> {
    let mut map = std::collections::BTreeMap::<u8, Vec<u8>>::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let tag = bytes[pos];
        let len = usize::from(bytes[pos + 1]);
        pos += 2;
        map.entry(tag)
            .or_default()
            .extend_from_slice(&bytes[pos..pos + len]);
        pos += len;
    }
    map
}
fn hk(secret: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut key = [0; 32];
    hkdf::Hkdf::<sha2_11::Sha512>::new(Some(salt), secret)
        .expand(info, &mut key)
        .unwrap();
    key
}
fn seal(key: [u8; 32], nonce: [u8; 12], aad: &[u8], mut data: Vec<u8>) -> Vec<u8> {
    ChaCha20Poly1305::new(&key.into())
        .encrypt_in_place(&Nonce::from(nonce), aad, &mut data)
        .unwrap();
    data
}
fn open(key: [u8; 32], nonce: [u8; 12], aad: &[u8], mut data: Vec<u8>) -> Vec<u8> {
    ChaCha20Poly1305::new(&key.into())
        .decrypt_in_place(&Nonce::from(nonce), aad, &mut data)
        .unwrap();
    data
}

fn modern_pair(s: &mut TcpStream, wrong_pin: bool) -> Option<[u8; 32]> {
    let mime = "application/pairing+tlv8";
    let (status, body) = exchange(
        s,
        "POST",
        "/pair-setup",
        &tlv(&[(6, &[1]), (0, &[0])]),
        mime,
    );
    assert_eq!(status, 200);
    let m2 = tags(&body);
    assert_eq!(m2[&6], [2]);
    let client = srp::ClientG3072::<sha2_11::Sha512>::new_with_options(true);
    let private = [7; 64];
    let public = client.compute_public_ephemeral(&private);
    let verifier = client
        .process_reply(
            &private,
            b"Pair-Setup",
            if wrong_pin {
                b"999-99-999"
            } else {
                b"264-82-648"
            },
            &m2[&2],
            &m2[&3],
        )
        .unwrap();
    let (status, body) = exchange(
        s,
        "POST",
        "/pair-setup",
        &tlv(&[(6, &[3]), (3, &public), (4, verifier.proof())]),
        mime,
    );
    assert_eq!(status, 200);
    let m4 = tags(&body);
    if wrong_pin {
        assert!(m4.contains_key(&7));
        return None;
    }
    let setup_key = verifier.verify_server(&m4[&4]).unwrap();
    let signing = SigningKey::from_bytes(&[3; 32]);
    let id = b"synthetic-homekit-peer";
    let x = hk(
        setup_key,
        b"Pair-Setup-Controller-Sign-Salt",
        b"Pair-Setup-Controller-Sign-Info",
    );
    let signature = signing.sign(&[x.as_slice(), id, signing.verifying_key().as_bytes()].concat());
    let secret = hk(
        setup_key,
        b"Pair-Setup-Encrypt-Salt",
        b"Pair-Setup-Encrypt-Info",
    );
    let m5 = seal(
        secret,
        *b"\0\0\0\0PS-Msg05",
        b"",
        tlv(&[
            (1, id),
            (3, signing.verifying_key().as_bytes()),
            (10, &signature.to_bytes()),
        ]),
    );
    let (status, body) = exchange(s, "POST", "/pair-setup", &tlv(&[(6, &[5]), (5, &m5)]), mime);
    assert_eq!(status, 200);
    let m6 = tags(&body);
    assert_eq!(m6[&6], [6]);
    let peer = tags(&open(secret, *b"\0\0\0\0PS-Msg06", b"", m6[&5].clone()));
    let receiver_key = SigningKey::from_bytes(&[1; 32]).verifying_key();
    assert_eq!(peer[&3], receiver_key.to_bytes());
    let x = hk(
        setup_key,
        b"Pair-Setup-Accessory-Sign-Salt",
        b"Pair-Setup-Accessory-Sign-Info",
    );
    receiver_key
        .verify_strict(
            &[x.as_slice(), &peer[&1], &peer[&3]].concat(),
            &Signature::from_slice(&peer[&10]).unwrap(),
        )
        .unwrap();

    let ephemeral = EphemeralSecret::random_from_rng(&mut rand10::rng());
    let public = PublicKey::from(&ephemeral);
    let (status, body) = exchange(
        s,
        "POST",
        "/pair-verify",
        &tlv(&[(6, &[1]), (3, public.as_bytes())]),
        mime,
    );
    assert_eq!(status, 200);
    let v2 = tags(&body);
    let receiver = PublicKey::from(<[u8; 32]>::try_from(v2[&3].as_slice()).unwrap());
    let shared = ephemeral.diffie_hellman(&receiver).to_bytes();
    let secret = hk(
        &shared,
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
    );
    let peer = tags(&open(secret, *b"\0\0\0\0PV-Msg02", b"", v2[&5].clone()));
    receiver_key
        .verify_strict(
            &[receiver.as_bytes().as_slice(), &peer[&1], public.as_bytes()].concat(),
            &Signature::from_slice(&peer[&10]).unwrap(),
        )
        .unwrap();
    let signature = signing.sign(&[public.as_bytes().as_slice(), id, receiver.as_bytes()].concat());
    let encrypted = seal(
        secret,
        *b"\0\0\0\0PV-Msg03",
        b"",
        tlv(&[(1, id), (10, &signature.to_bytes())]),
    );
    let (status, body) = exchange(
        s,
        "POST",
        "/pair-verify",
        &tlv(&[(6, &[3]), (5, &encrypted)]),
        mime,
    );
    assert_eq!(status, 200);
    assert_eq!(tags(&body)[&6], [4]);
    Some(shared)
}

#[test]
fn modern_homekit_pairing_upgrades_control_and_rejects_tampered_ciphertext() {
    let (engine, _rx, port) = start();
    let mut s = socket(port);
    let shared = modern_pair(&mut s, false).unwrap();
    let write = hk(&shared, b"Control-Salt", b"Control-Write-Encryption-Key");
    let read = hk(&shared, b"Control-Salt", b"Control-Read-Encryption-Key");
    let message = b"GET /info RTSP/1.0\r\nCSeq: 9\r\nContent-Length: 0\r\n\r\n";
    let aad = (message.len() as u16).to_le_bytes();
    let encrypted = seal(write, [0; 12], &aad, message.to_vec());
    s.write_all(&aad).unwrap();
    s.write_all(&encrypted).unwrap();
    let mut aad = [0; 2];
    s.read_exact(&mut aad).unwrap();
    let mut bytes = vec![0; usize::from(u16::from_le_bytes(aad)) + 16];
    s.read_exact(&mut bytes).unwrap();
    let plain = open(read, [0; 12], &aad, bytes);
    assert!(plain.starts_with(b"RTSP/1.0 200"));
    assert!(plain.windows(8).any(|v| v == b"bplist00"));
    let mut nonce = [0; 12];
    nonce[4] = 1;
    let aad = (message.len() as u16).to_le_bytes();
    let mut bad = seal(write, nonce, &aad, message.to_vec());
    bad[3] ^= 1;
    s.write_all(&aad).unwrap();
    s.write_all(&bad).unwrap();
    let mut out = [0];
    assert!(matches!(s.read(&mut out), Ok(0) | Err(_)));
    engine.stop();
}
#[test]
fn wrong_modern_pin_does_not_fall_back_to_legacy_pairing() {
    let (engine, _rx, port) = start();
    let mut s = socket(port);
    assert!(modern_pair(&mut s, true).is_none());
    let mut byte = [0];
    assert!(matches!(s.read(&mut byte), Ok(0) | Err(_)));
    engine.stop();
}

struct EncryptedPeer<'a> {
    socket: &'a mut TcpStream,
    write: [u8; 32],
    read: [u8; 32],
    tx: u64,
    rx: u64,
}
impl<'a> EncryptedPeer<'a> {
    fn new(socket: &'a mut TcpStream, shared: [u8; 32]) -> Self {
        Self {
            socket,
            write: hk(&shared, b"Control-Salt", b"Control-Write-Encryption-Key"),
            read: hk(&shared, b"Control-Salt", b"Control-Read-Encryption-Key"),
            tx: 0,
            rx: 0,
        }
    }
    fn send(&mut self, method: &str, path: &str, value: &plist::Value) {
        let mut body = Vec::new();
        plist::to_writer_binary(&mut body, value).unwrap();
        self.send_bytes(method, path, &body, "application/x-apple-binary-plist");
    }
    fn send_bytes(&mut self, method: &str, path: &str, body: &[u8], kind: &str) {
        let header = format!(
            "{method} {path} RTSP/1.0\r\nCSeq: 10\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let bytes = [header.as_bytes(), body].concat();
        for chunk in bytes.chunks(1024) {
            let len = (chunk.len() as u16).to_le_bytes();
            let mut nonce = [0; 12];
            nonce[4..].copy_from_slice(&self.tx.to_le_bytes());
            self.tx += 1;
            self.socket.write_all(&len).unwrap();
            self.socket
                .write_all(&seal(self.write, nonce, &len, chunk.to_vec()))
                .unwrap();
        }
    }
    fn response(&mut self) -> plist::Value {
        plist::from_bytes(&self.response_bytes()).unwrap()
    }
    fn response_bytes(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut len = [0; 2];
            self.socket.read_exact(&mut len).unwrap();
            let mut block = vec![0; usize::from(u16::from_le_bytes(len)) + 16];
            self.socket.read_exact(&mut block).unwrap();
            let mut nonce = [0; 12];
            nonce[4..].copy_from_slice(&self.rx.to_le_bytes());
            self.rx += 1;
            bytes.extend_from_slice(&open(self.read, nonce, &len, block));
            if let Some(head) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                let head = head + 4;
                let headers = std::str::from_utf8(&bytes[..head]).unwrap();
                assert!(headers.starts_with("RTSP/1.0 200"), "{headers}");
                let len = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= head + len {
                    return bytes[head..head + len].to_vec();
                }
            }
            assert!(bytes.len() < 1024 * 1024);
        }
    }
}
struct TimingResponder {
    port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl TimingResponder {
    fn start() -> Self {
        let socket = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut bytes = [0; 64];
            while !done.load(std::sync::atomic::Ordering::Acquire) {
                if let Ok((32, peer)) = socket.recv_from(&mut bytes) {
                    let mut reply = [0; 32];
                    reply[0] = 0x80;
                    reply[1] = 0xd3;
                    reply[2..4].copy_from_slice(&bytes[2..4]);
                    reply[8..16].copy_from_slice(&bytes[24..32]);
                    reply[16..24].copy_from_slice(&rairplay::timing::now_fixed().to_be_bytes());
                    reply[24..32].copy_from_slice(&rairplay::timing::now_fixed().to_be_bytes());
                    socket.send_to(&reply, peer).unwrap();
                }
            }
        });
        Self {
            port,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for TimingResponder {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn dict(entries: Vec<(&str, plist::Value)>) -> plist::Value {
    plist::Value::Dictionary(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}
fn integer(n: u64) -> plist::Value {
    plist::Value::Integer(n.into())
}
fn video_packet(
    socket: &mut TcpStream,
    kind: u16,
    data: &[u8],
    encryption: Option<([u8; 32], u64)>,
) {
    let mut header = [0; 128];
    let size = data.len() + if encryption.is_some() { 16 } else { 0 };
    header[..4].copy_from_slice(&(size as u32).to_le_bytes());
    header[4..6].copy_from_slice(&kind.to_le_bytes());
    header[8..16].copy_from_slice(&rairplay::timing::now_fixed().to_le_bytes());
    let payload = if let Some((key, count)) = encryption {
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&count.to_le_bytes());
        seal(key, nonce, &header, data.to_vec())
    } else {
        data.to_vec()
    };
    socket.write_all(&header).unwrap();
    socket.write_all(&payload).unwrap();
}
#[test]
fn modern_encrypted_media_reaches_bounded_host_callbacks_with_protocol_timestamps() {
    let (engine, rx, port) = start();
    let timing = TimingResponder::start();
    let mut socket = socket(port);
    let shared = modern_pair(&mut socket, false).unwrap();
    let mut control = EncryptedPeer::new(&mut socket, shared);
    let info = dict(vec![
        ("name", "Synthetic sender".into()),
        ("model", "Fixture".into()),
        ("deviceID", "fixture".into()),
        ("macAddress", "02:00:00:00:00:01".into()),
        ("timingProtocol", "NTP".into()),
        ("timingPort", integer(u64::from(timing.port))),
    ]);
    control.send("SETUP", "/", &info);
    let Event::Request { session, .. } = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("approval required")
    };
    engine.host.approve(session, true);
    let info = control.response();
    assert!(
        info.as_dictionary().unwrap()["timingPort"]
            .as_unsigned_integer()
            .unwrap()
            > 0
    );
    control.send(
        "SETUP",
        "/42",
        &dict(vec![(
            "streams",
            plist::Value::Array(vec![dict(vec![
                ("type", integer(110)),
                ("streamConnectionID", integer(9)),
                ("latencyMs", integer(0)),
            ])]),
        )]),
    );
    let setup = control.response();
    let video_port = setup.as_dictionary().unwrap()["streams"]
        .as_array()
        .unwrap()[0]
        .as_dictionary()
        .unwrap()["dataPort"]
        .as_unsigned_integer()
        .unwrap() as u16;
    let mut video = TcpStream::connect((Ipv4Addr::LOCALHOST, video_port)).unwrap();
    video_packet(
        &mut video,
        1,
        include_bytes!("fixtures/rx580-config.avcc"),
        None,
    );
    let Event::VideoConfig { config, .. } = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("video configuration not delivered")
    };
    assert_eq!((config.width, config.height), (1280, 720));
    let key = hk(
        &shared,
        b"DataStream-Salt9",
        b"DataStream-Output-Encryption-Key",
    );
    let mut delivered = false;
    for count in 0..40 {
        video_packet(
            &mut video,
            0,
            include_bytes!("fixtures/rx580-frame.avc"),
            Some((key, count)),
        );
        if let Ok(Event::Video { data, pts, key, .. }) = rx.recv_timeout(Duration::from_millis(100))
        {
            assert!(key);
            assert_eq!(
                data,
                config
                    .annex_b(include_bytes!("fixtures/rx580-frame.avc"))
                    .unwrap()
                    .0
            );
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros() as i64;
            assert!((now - pts).abs() < 1_000_000);
            delivered = true;
            break;
        }
    }
    assert!(delivered, "NTP-synchronized video was not delivered");
    control.send(
        "SETUP",
        "/42",
        &dict(vec![(
            "streams",
            plist::Value::Array(vec![dict(vec![
                ("type", integer(96)),
                ("streamConnectionID", integer(10)),
                ("audioFormat", integer(1 << 24)),
                ("spf", integer(480)),
            ])]),
        )]),
    );
    let setup = control.response();
    let stream = setup.as_dictionary().unwrap()["streams"]
        .as_array()
        .unwrap()[0]
        .as_dictionary()
        .unwrap();
    let audio_port = stream["dataPort"].as_unsigned_integer().unwrap() as u16;
    let control_port = stream["controlPort"].as_unsigned_integer().unwrap() as u16;
    let Event::AudioConfig {
        codec, rate, spf, ..
    } = rx.recv_timeout(Duration::from_secs(3)).unwrap()
    else {
        panic!("audio configuration not delivered")
    };
    assert_eq!((codec, rate, spf), (2, 44100, 480));
    let udp = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let mut sync = [0u8; 20];
    sync[0] = 0x80;
    sync[1] = 0xd4;
    sync[4..8].copy_from_slice(&1000u32.to_be_bytes());
    sync[8..16].copy_from_slice(&rairplay::timing::now_fixed().to_be_bytes());
    udp.send_to(&sync, (Ipv4Addr::LOCALHOST, control_port))
        .unwrap();
    let audio_key = hk(
        &shared,
        b"DataStream-Salt10",
        b"DataStream-Output-Encryption-Key",
    );
    let mut delivered = false;
    // This payload checks authenticated delivery/profile routing, not an AAC decoder.
    for sequence in 1..30u16 {
        let mut rtp = vec![0u8; 12];
        rtp[0] = 0x80;
        rtp[1] = 0x60;
        rtp[2..4].copy_from_slice(&sequence.to_be_bytes());
        rtp[4..8].copy_from_slice(&(1000 + u32::from(sequence) * 480).to_be_bytes());
        let mut nonce = [0; 12];
        nonce[4..].copy_from_slice(&u64::from(sequence).to_le_bytes());
        let encrypted = seal(audio_key, nonce, &rtp[4..12], vec![1, 2, 3, 4, 5]);
        rtp.extend(encrypted);
        rtp.extend_from_slice(&nonce[4..]);
        udp.send_to(&rtp, (Ipv4Addr::LOCALHOST, audio_port))
            .unwrap();
        if let Ok(Event::Audio { data, pts, .. }) = rx.recv_timeout(Duration::from_millis(50)) {
            assert_eq!(data, [1, 2, 3, 4, 5]);
            assert!(pts > 0);
            delivered = true;
            break;
        }
    }
    assert!(delivered, "synchronized audio was not delivered");
    control.send_bytes(
        "SET_PARAMETER",
        "/42",
        b"volume: -12.5\r\n",
        "text/parameters",
    );
    assert!(control.response_bytes().is_empty());
    let Event::Volume { db, .. } = rx.recv_timeout(Duration::from_secs(3)).unwrap() else {
        panic!("volume was not routed")
    };
    assert_eq!(db, -12.5);
    engine.stop();
}
