use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rairplay::config::Keychain;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    sync::Mutex,
};

/// The platform stores the random seed in private, non-backed-up storage.
/// Trusted peer public keys persist; every media connection still needs TV approval.
pub struct Identity {
    signing: SigningKey,
    public: [u8; 32],
    id: String,
    peers: Mutex<BTreeMap<Vec<u8>, [u8; 32]>>,
    storage: Option<PathBuf>,
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
            storage: None,
        }
    }

    pub fn device_id(&self) -> &str {
        &self.id
    }
    pub fn load(seed: [u8; 32], storage: Option<PathBuf>) -> io::Result<Self> {
        let mut identity = Self::new(seed);
        if let Some(path) = &storage {
            match std::fs::metadata(path) {
                Ok(meta) => {
                    if meta.len() > 16384 {
                        return Err(io::Error::other("peer store limit"));
                    }
                    let store: PeerStore = serde_json::from_slice(&std::fs::read(path)?)?;
                    if store.device != identity.id || store.peers.len() > 16 {
                        return Err(io::Error::other("peer store identity"));
                    }
                    let peers = identity.peers.get_mut().unwrap();
                    for (id, key) in store.peers {
                        let id = hex::decode(id).map_err(io::Error::other)?;
                        let key: [u8; 32] = hex::decode(key)
                            .map_err(io::Error::other)?
                            .try_into()
                            .map_err(|_| io::Error::other("peer key length"))?;
                        if id.is_empty()
                            || id.len() > 128
                            || VerifyingKey::from_bytes(&key).is_err()
                            || peers.insert(id, key).is_some()
                        {
                            return Err(io::Error::other("invalid peer record"));
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        identity.storage = storage;
        Ok(identity)
    }
    fn save(&self, peers: &BTreeMap<Vec<u8>, [u8; 32]>) -> io::Result<()> {
        let Some(path) = &self.storage else {
            return Ok(());
        };
        let store = PeerStore {
            device: self.id.clone(),
            peers: peers
                .iter()
                .map(|(id, key)| (hex::encode(id), hex::encode(key)))
                .collect(),
        };
        let temp = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(&store)?)?;
        file.sync_all()?;
        std::fs::rename(temp, path)
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
        if self.save(&peers).is_err() {
            peers.remove(id);
            return false;
        }
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
    #[test]
    fn trusted_peers_survive_restart_and_corrupt_store_is_not_silently_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let peer = Identity::new([2; 32]);
        let a = Identity::load([1; 32], Some(path.clone())).unwrap();
        assert!(a.trust(b"peer", peer.pubkey()));
        drop(a);
        let b = Identity::load([1; 32], Some(path.clone())).unwrap();
        assert!(b.verify(b"peer", b"hello", &peer.sign(b"hello")));
        assert!(Identity::load([9; 32], Some(path.clone())).is_err());
        std::fs::write(&path, b"corrupt").unwrap();
        assert!(Identity::load([1; 32], Some(path)).is_err());
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PeerStore {
    device: String,
    peers: Vec<(String, String)>,
}
