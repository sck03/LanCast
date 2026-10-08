use std::sync::Mutex;

use super::handlers::inner::State as InnerState;

pub struct ServiceState {
    pub pairing: Mutex<InnerState>,
    pub pending_key: Mutex<Option<[u8; 32]>>,
}

impl ServiceState {
    pub fn new(public: Vec<u8>, signer: impl Fn(&[u8]) -> Vec<u8> + Send + Sync + 'static) -> Self {
        Self {
            pairing: Mutex::new(InnerState::new(public, signer)),
            pending_key: Mutex::new(None),
        }
    }
}
