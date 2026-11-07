//! Bounded, versioned local compatibility evidence. Discovery and HTTP pulls never imply success.
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::PathBuf};

const SCHEMA: u32 = 2;
const TTL: u64 = 30 * 24 * 60 * 60;
const MAX_DEVICES: usize = 256;
const MAX_PROFILES_PER_DEVICE: usize = 16;
const MAX_BYTES: usize = 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Evidence {
    pub signature: String,
    pub profile: String,
    pub confirmed_at: u64,
    pub passed: bool,
    pub source: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema_version: u32,
    devices: BTreeMap<String, BTreeMap<String, Evidence>>,
}
impl Document {
    fn read(reader: impl Read) -> anyhow::Result<Self> {
        // Bound the actual read, including files that grow after opening.
        let mut bytes = Vec::new();
        reader
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= MAX_BYTES, "PROFILE_FILE_TOO_LARGE");
        let document: Self = serde_json::from_slice(&bytes)?;
        ensure!(
            document.schema_version == SCHEMA
                && document.devices.len() <= MAX_DEVICES
                && document
                    .devices
                    .values()
                    .all(|profiles| profiles.len() <= MAX_PROFILES_PER_DEVICE),
            "PROFILE_SCHEMA_UNSUPPORTED"
        );
        Ok(document)
    }
}
pub struct ProfileStore {
    path: PathBuf,
    document: Document,
}
impl ProfileStore {
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        Self::open_checked(path, || Ok(()))
    }
    pub(crate) fn open_checked(
        path: PathBuf,
        check: impl Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        check()?;
        ensure!(path.is_absolute(), "ABSOLUTE_PROFILE_PATH_REQUIRED");
        let document = match std::fs::File::open(&path) {
            Ok(file) => {
                let metadata = file.metadata()?;
                ensure!(metadata.is_file(), "INVALID_PROFILE_FILE");
                ensure!(metadata.len() <= MAX_BYTES as u64, "PROFILE_FILE_TOO_LARGE");
                Document::read(file)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Document {
                schema_version: SCHEMA,
                devices: BTreeMap::new(),
            },
            Err(error) => return Err(error.into()),
        };
        check()?;
        Ok(Self { path, document })
    }
    pub fn get(&self, id: &str, signature: &str, profile: &str, now: u64) -> Option<&Evidence> {
        self.document.devices.get(id)?.get(profile).filter(|e| {
            e.signature == signature
                && e.profile == profile
                && now >= e.confirmed_at
                && now - e.confirmed_at <= TTL
                && e.source == "user_confirmed_synthetic"
        })
    }
    pub fn save(&mut self, id: String, evidence: Evidence) -> anyhow::Result<()> {
        self.save_checked(id, evidence, || Ok(()))
    }
    pub(crate) fn save_checked(
        &mut self,
        id: String,
        evidence: Evidence,
        check: impl Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        check()?;
        ensure!(
            id.len() <= 256 && evidence.signature.len() <= 256 && evidence.profile.len() <= 128,
            "INVALID_PROFILE"
        );
        let mut document = self.document.clone();
        let now = evidence.confirmed_at;
        document.devices.retain(|_, profiles| {
            profiles.retain(|_, evidence| {
                !(now >= evidence.confirmed_at && now - evidence.confirmed_at > TTL)
            });
            !profiles.is_empty()
        });
        ensure!(
            document.devices.contains_key(&id) || document.devices.len() < MAX_DEVICES,
            "PROFILE_LIMIT_REACHED"
        );
        let profiles = document.devices.entry(id).or_default();
        ensure!(
            profiles.contains_key(&evidence.profile) || profiles.len() < MAX_PROFILES_PER_DEVICE,
            "PROFILE_LIMIT_REACHED"
        );
        profiles.insert(evidence.profile.clone(), evidence);
        self.persist(&document, &check)?;
        self.document = document;
        Ok(())
    }
    fn persist(
        &self,
        document: &Document,
        check: &impl Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        check()?;
        let bytes = serde_json::to_vec_pretty(document)?;
        ensure!(bytes.len() <= MAX_BYTES, "PROFILE_FILE_TOO_LARGE");
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
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            // Do not publish a cancelled command's temporary snapshot.
            check()?;
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
        let mut store = ProfileStore::open(path.clone()).unwrap();
        assert!(
            store
                .get("tv", "firmware1", "720p-aac", 1001)
                .unwrap()
                .passed
        );
        store
            .save(
                "tv".into(),
                Evidence {
                    signature: "firmware1".into(),
                    profile: "720p-muted".into(),
                    confirmed_at: 1000,
                    passed: true,
                    source: "user_confirmed_synthetic".into(),
                },
            )
            .unwrap();
        assert!(store.get("tv", "firmware1", "720p-aac", 1001).is_some());
        assert!(store.get("tv", "firmware1", "720p-muted", 1001).is_some());
        for (s, p, t) in [
            ("firmware2", "720p-aac", 1001),
            ("firmware1", "unsupported", 1001),
            ("firmware1", "720p-aac", 999),
            ("firmware1", "720p-aac", 1001 + TTL),
        ] {
            assert!(store.get("tv", s, p, t).is_none());
        }
        std::fs::write(&path, br#"{"schemaVersion":99,"devices":{}}"#).unwrap();
        assert!(ProfileStore::open(path.clone()).is_err());
        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"devices":{"old":{"signature":"firmware1","profile":"720p-aac","confirmedAt":1000,"passed":true,"source":"user_confirmed_synthetic"}}}"#,
        )
        .unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(ProfileStore::open(path.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn save_prunes_expired_devices_before_enforcing_the_limit() {
        let path = std::env::temp_dir().join(format!(
            "lancast-profile-limit-{}.json",
            uuid::Uuid::new_v4()
        ));
        let mut store = ProfileStore::open(path.clone()).unwrap();
        for n in 0..MAX_DEVICES {
            store.document.devices.insert(
                format!("old-{n}"),
                BTreeMap::from([(
                    "profile".into(),
                    Evidence {
                        signature: "sig".into(),
                        profile: "profile".into(),
                        confirmed_at: 0,
                        passed: false,
                        source: "user_confirmed_synthetic".into(),
                    },
                )]),
            );
        }
        store
            .save(
                "new".into(),
                Evidence {
                    signature: "sig".into(),
                    profile: "profile".into(),
                    confirmed_at: now(),
                    passed: true,
                    source: "user_confirmed_synthetic".into(),
                },
            )
            .unwrap();
        assert!(store.get("new", "sig", "profile", now()).is_some());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cancelled_save_keeps_disk_and_memory_at_the_previous_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profiles.json");
        let mut store = ProfileStore::open(path.clone()).unwrap();
        let evidence = Evidence {
            signature: "firmware".into(),
            profile: "with-audio".into(),
            confirmed_at: now(),
            passed: true,
            source: "user_confirmed_synthetic".into(),
        };
        store.save("tv".into(), evidence.clone()).unwrap();
        let original = std::fs::read(&path).unwrap();
        let checks = std::cell::Cell::new(0);
        let mut replacement = evidence;
        // Pruning must be part of the transaction too: cancellation must retain
        // the previous snapshot even when the attempted save advances its clock.
        replacement.confirmed_at += TTL + 1;
        replacement.passed = false;
        let result = store.save_checked("tv".into(), replacement, || {
            checks.set(checks.get() + 1);
            ensure!(checks.get() < 3, "COMMAND_CANCELLED");
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(
            store
                .get("tv", "firmware", "with-audio", now())
                .unwrap()
                .passed
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn document_read_stops_at_the_byte_budget() {
        let mut reader = std::io::Cursor::new(vec![b' '; MAX_BYTES * 2]);
        assert!(
            Document::read(&mut reader)
                .err()
                .unwrap()
                .to_string()
                .contains("PROFILE_FILE_TOO_LARGE")
        );
        assert_eq!(reader.position(), (MAX_BYTES + 1) as u64);
    }
}
