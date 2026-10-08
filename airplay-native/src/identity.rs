use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rairplay::config::Keychain;
use std::{collections::BTreeMap, sync::Mutex};

/// The platform stores the random seed in private, non-backed-up storage.
/// Peer trust lasts one explicit enable cycle; every connection also needs TV approval.
pub struct Identity {
    signing: SigningKey,
    public: [u8; 32],
    id: String,
    peers: Mutex<BTreeMap<Vec<u8>, [u8; 32]>>,
}

impl Identity {
    pub fn new(seed: [u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(&seed);
        let public = signing.verifying_key().to_bytes();
        let mut mac = public[..6].to_vec();
        mac[0] = (mac[0] | 2) & !1; // Locally administered unicast, never a hardware MAC.
        let id = mac
            .iter()
            .map(|v| format!("{v:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        Self {
            signing,
            public,
            id,
            peers: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn device_id(&self) -> &str {
        &self.id
    }
    pub fn public_hex(&self) -> String {
        hex::encode(self.public)
    }
}

impl Keychain for Identity {
    fn id(&self) -> &[u8] {
        self.id.as_bytes()
    }
    fn pubkey(&self) -> &[u8] {
        &self.public
    }
    fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.signing.sign(data).to_bytes().to_vec()
    }
    fn trust(&self, id: &[u8], key: &[u8]) -> bool {
        if id.is_empty() || id.len() > 128 {
            return false;
        }
        let Ok(key) = <[u8; 32]>::try_from(key) else {
            return false;
        };
        if VerifyingKey::from_bytes(&key).is_err() {
            return false;
        }
        let mut peers = self.peers.lock().unwrap();
        if let Some(old) = peers.get(id) {
            return old == &key;
        }
        if peers.len() >= 16 {
            return false;
        }
        peers.insert(id.to_vec(), key);
        true
    }
    fn verify(&self, id: &[u8], message: &[u8], signature: &[u8]) -> bool {
        let peers = self.peers.lock().unwrap();
        let Some(key) = peers.get(id) else {
            return false;
        };
        let (Ok(key), Ok(sig)) = (
            VerifyingKey::from_bytes(key),
            Signature::from_slice(signature),
        ) else {
            return false;
        };
        key.verify_strict(message, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_stable_but_not_shared_and_trust_cannot_be_replaced() {
        let a = Identity::new([1; 32]);
        let b = Identity::new([2; 32]);
        assert_eq!(a.pubkey(), Identity::new([1; 32]).pubkey());
        assert_ne!(a.pubkey(), b.pubkey());
        assert!(!a.verify(b"peer", b"hello", &b.sign(b"hello")));
        assert!(a.trust(b"peer", b.pubkey()));
        assert!(a.verify(b"peer", b"hello", &b.sign(b"hello")));
        assert!(!a.verify(b"peer", b"changed", &b.sign(b"hello")));
        assert!(!a.trust(b"peer", a.pubkey()));
    }
}
