//! Shared discovery limits, independent of protocol I/O and host event delivery.
use serde::Serialize;
use std::collections::HashMap;

pub(super) const MAX_DEVICES: usize = 64;
// Leave room in the 128 KiB host event for its envelope and scan generation.
pub(super) const MAX_RESULT_BYTES: usize = 96 * 1024;

pub(super) struct Results<T> {
    entries: HashMap<String, (T, usize)>,
    bytes: usize,
}
impl<T: Serialize> Results<T> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 2, // Array brackets; every entry also reserves a separator.
        }
    }
    pub fn is_full(&self) -> bool {
        self.entries.len() == MAX_DEVICES
    }
    pub fn insert(&mut self, id: String, device: T) {
        let Ok(size) = crate::protocol::json_size(&device, MAX_RESULT_BYTES) else {
            return;
        };
        let size = size + 1;
        let previous = self.entries.get(&id).map_or(0, |(_, size)| *size);
        if (previous == 0 && self.is_full()) || size > MAX_RESULT_BYTES - (self.bytes - previous) {
            return;
        }
        self.bytes = self.bytes - previous + size;
        self.entries.insert(id, (device, size));
    }
    pub fn into_devices(self) -> Vec<T> {
        self.entries
            .into_values()
            .map(|(device, _)| device)
            .collect()
    }
}
