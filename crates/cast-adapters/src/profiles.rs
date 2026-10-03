//! Bounded, versioned local compatibility evidence. Discovery and HTTP pulls never imply success.
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

const SCHEMA: u32 = 1;
const TTL: u64 = 30 * 24 * 60 * 60;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Evidence {
    pub signature: String,
    pub profile: String,
    pub confirmed_at: u64,
    pub passed: bool,
    pub source: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema_version: u32,
    devices: BTreeMap<String, Evidence>,
}
pub struct ProfileStore {
    path: PathBuf,
    document: Document,
}
impl ProfileStore {
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        ensure!(path.is_absolute(), "ABSOLUTE_PROFILE_PATH_REQUIRED");
        let document = if path.exists() {
            ensure!(
                std::fs::metadata(&path)?.len() <= 1024 * 1024,
                "PROFILE_FILE_TOO_LARGE"
            );
            let value: Document = serde_json::from_slice(&std::fs::read(&path)?)?;
            ensure!(
                value.schema_version == SCHEMA && value.devices.len() <= 256,
                "PROFILE_SCHEMA_UNSUPPORTED"
            );
            value
        } else {
            Document {
                schema_version: SCHEMA,
                devices: BTreeMap::new(),
            }
        };
        Ok(Self { path, document })
    }
    pub fn get(&self, id: &str, signature: &str, profile: &str, now: u64) -> Option<&Evidence> {
        self.document.devices.get(id).filter(|e| {
            e.signature == signature
                && e.profile == profile
                && now >= e.confirmed_at
                && now - e.confirmed_at <= TTL
                && e.source == "user_confirmed_synthetic"
        })
    }
    pub fn save(&mut self, id: String, evidence: Evidence) -> anyhow::Result<()> {
        ensure!(
            id.len() <= 256 && evidence.signature.len() <= 256 && evidence.profile.len() <= 128,
            "INVALID_PROFILE"
        );
        ensure!(
            self.document.devices.contains_key(&id) || self.document.devices.len() < 256,
            "PROFILE_LIMIT_REACHED"
        );
        let old = self.document.devices.insert(id.clone(), evidence);
        let result = self.persist();
        if result.is_err() {
            if let Some(old) = old {
                self.document.devices.insert(id, old);
            } else {
                self.document.devices.remove(&id);
            }
        }
        result
    }
    fn persist(&self) -> anyhow::Result<()> {
        let parent = self.path.parent().context("INVALID_PROFILE_PATH")?;
        std::fs::create_dir_all(parent)?;
        // A unique temporary file prevents one process from truncating another writer's temporary file.
        let temporary = parent.join(format!(".profiles-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(&self.document)?)?;
            file.sync_all()?;
            drop(file);
            // Windows rename does not replace an existing destination. ReplaceFile preserves the old
            // document on failure; on other platforms rename is the atomic replacement primitive.
            #[cfg(windows)]
            if self.path.exists() {
                use std::os::windows::ffi::OsStrExt;
                unsafe extern "system" {
                    fn ReplaceFileW(
                        old: *const u16,
                        new: *const u16,
                        backup: *const u16,
                        flags: u32,
                        exclude: *mut core::ffi::c_void,
                        reserved: *mut core::ffi::c_void,
                    ) -> i32;
                }
                let old: Vec<u16> = self.path.as_os_str().encode_wide().chain(Some(0)).collect();
                let new: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
                if unsafe {
                    ReplaceFileW(
                        old.as_ptr(),
                        new.as_ptr(),
                        std::ptr::null(),
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                } == 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            } else {
                std::fs::rename(&temporary, &self.path)?;
            }
            #[cfg(not(windows))]
            std::fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evidence_expires_and_never_crosses_device_configuration() {
        let path =
            std::env::temp_dir().join(format!("lancast-profile-{}.json", uuid::Uuid::new_v4()));
        let mut store = ProfileStore::open(path.clone()).unwrap();
        store
            .save(
                "tv".into(),
                Evidence {
                    signature: "firmware1".into(),
                    profile: "720p-aac".into(),
                    confirmed_at: 1000,
                    passed: true,
                    source: "user_confirmed_synthetic".into(),
                },
            )
            .unwrap();
        let store = ProfileStore::open(path.clone()).unwrap();
        assert!(
            store
                .get("tv", "firmware1", "720p-aac", 1001)
                .unwrap()
                .passed
        );
        for (s, p, t) in [
            ("firmware2", "720p-aac", 1001),
            ("firmware1", "720p-muted", 1001),
            ("firmware1", "720p-aac", 999),
            ("firmware1", "720p-aac", 1001 + TTL),
        ] {
            assert!(store.get("tv", s, p, t).is_none());
        }
        std::fs::write(&path, br#"{"schemaVersion":99,"devices":{}}"#).unwrap();
        assert!(ProfileStore::open(path.clone()).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
