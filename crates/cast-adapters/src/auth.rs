use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use p256::{
    ecdsa::{
        Signature, SigningKey, VerifyingKey,
        signature::{Signer, Verifier},
    },
    pkcs8::{DecodePrivateKey, DecodePublicKey},
};
use rand::RngCore;
use rcgen::{CertificateParams, KeyPair, PublicKeyData};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

/// All current senders and receivers must use the same pairing dialect.
pub const PAIRING_VERSION: u64 = 2;
pub fn valid_pairing_code(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn random_token() -> String {
    let mut data = [0u8; 32];
    rand::rng().fill_bytes(&mut data);
    STANDARD.encode(data)
}
pub fn same_secret(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Ephemeral identity: no credentials are silently persisted in plaintext.
pub struct Identity {
    pub id: Uuid,
    pub cert: CertificateDer<'static>,
    pub key: Vec<u8>,
    pub public_key: String,
    pub fingerprint: String,
}
impl Identity {
    pub fn create() -> anyhow::Result<Self> {
        let key = KeyPair::generate()?;
        let public = key.subject_public_key_info();
        let cert = CertificateParams::new(vec!["lancast.local".into()])?.self_signed(&key)?;
        Ok(Self {
            id: Uuid::new_v4(),
            cert: cert.der().clone(),
            key: key.serialize_der(),
            public_key: STANDARD.encode(&public),
            fingerprint: hex::encode(Sha256::digest(&public)),
        })
    }
    pub fn sign(&self, nonce: &str) -> anyhow::Result<String> {
        let key = SigningKey::from_pkcs8_der(&self.key)?;
        let signature: Signature = key.sign(&challenge_bytes(nonce, self.id)?);
        Ok(STANDARD.encode(signature.to_der().as_bytes()))
    }
    pub fn server_config(&self) -> anyhow::Result<rustls::ServerConfig> {
        Ok(rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![self.cert.clone()],
                PrivateKeyDer::try_from(self.key.clone()).map_err(anyhow::Error::msg)?,
            )?)
    }
}
fn challenge_bytes(nonce: &str, id: Uuid) -> anyhow::Result<Vec<u8>> {
    let nonce = STANDARD.decode(nonce)?;
    ensure!(nonce.len() == 32, "INVALID_NONCE");
    let mut bytes = b"LanCast-auth-v1".to_vec();
    bytes.extend(nonce);
    bytes.extend(id.as_bytes());
    Ok(bytes)
}
pub fn verify(public: &str, nonce: &str, id: Uuid, signature: &str) -> anyhow::Result<()> {
    let key = VerifyingKey::from_public_key_der(&STANDARD.decode(public)?)?;
    let signature = Signature::from_der(&STANDARD.decode(signature)?)?;
    key.verify(&challenge_bytes(nonce, id)?, &signature)
        .context("AUTH_REQUIRED")
}
pub struct Invitation {
    token: String,
    created: Instant,
    attempts: u8,
    used: bool,
}
impl Invitation {
    pub fn new(now: Instant) -> Self {
        Self {
            // Human-entered, single-use code. Five attempts, two-minute expiry
            // and explicit receiver approval remain mandatory. Never advertise it.
            token: format!("{:08}", rand::random_range(0..100_000_000u32)),
            created: now,
            attempts: 0,
            used: false,
        }
    }
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn consume(&mut self, token: &str, now: Instant) -> anyhow::Result<()> {
        ensure!(
            !self.used
                && self.attempts < 5
                && now.duration_since(self.created) < Duration::from_secs(120),
            "PAIR_EXPIRED"
        );
        self.attempts += 1;
        ensure!(same_secret(&self.token, token), "PAIR_REJECTED");
        self.used = true;
        Ok(())
    }
}

#[derive(Debug)]
struct PinVerifier {
    expected: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
            .map_err(|_| rustls::Error::General("invalid certificate".into()))?;
        let digest: [u8; 32] = Sha256::digest(parsed.public_key().raw).into();
        use subtle::ConstantTimeEq;
        if bool::from(digest.ct_eq(&self.expected)) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("TLS_PIN_MISMATCH".into()))
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub fn pinned_config(fingerprint: &str) -> anyhow::Result<rustls::ClientConfig> {
    let expected: [u8; 32] = hex::decode(fingerprint.replace(':', ""))?
        .try_into()
        .map_err(|_| anyhow::anyhow!("INVALID_FINGERPRINT"))?;
    Ok(rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier {
            expected,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        }))
        .with_no_client_auth())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature_binds_challenge_and_device() {
        let a = Identity::create().unwrap();
        let nonce = random_token();
        let sig = a.sign(&nonce).unwrap();
        verify(&a.public_key, &nonce, a.id, &sig).unwrap();
        assert!(verify(&a.public_key, &random_token(), a.id, &sig).is_err());
        assert!(verify(&a.public_key, &nonce, Uuid::new_v4(), &sig).is_err());
    }
    #[test]
    fn invitation_single_use_and_attempt_limit() {
        let now = Instant::now();
        let mut i = Invitation::new(now);
        let token = i.token.clone();
        assert_eq!(token.len(), 8);
        assert!(token.bytes().all(|b| b.is_ascii_digit()));
        assert!(valid_pairing_code(&token));
        assert!(!valid_pairing_code(&random_token()));
        assert!(!valid_pairing_code("1234abcd"));
        i.consume(&token, now).unwrap();
        assert!(i.consume(&token, now).is_err());
        let mut i = Invitation::new(now);
        for _ in 0..5 {
            assert!(i.consume("wrong", now).is_err());
        }
        assert!(i.consume(&i.token.clone(), now).is_err());
        let mut expired = Invitation::new(now);
        assert!(
            expired
                .consume(&expired.token.clone(), now + Duration::from_secs(120))
                .is_err()
        );
    }
}
