//! DLNA profile evidence and live grant preparation; transport remains in its adapters.
use super::{LiveContext, Runtime, string};
use crate::{dlna, events::event};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::{
    net::IpAddr,
    sync::{Arc, Mutex as StdMutex},
};

impl Runtime {
    pub(super) async fn live_command(
        &mut self,
        c: Value,
        command_generation: u64,
    ) -> anyhow::Result<()> {
        match string(&c, "op")? {
            "profiles.open" => {
                ensure!(self.profiles.is_none(), "PROFILES_ALREADY_OPEN");
                let path = string(&c, "path")?.into();
                let profiles = self
                    .io
                    .run(command_generation, move |guard| {
                        crate::profiles::ProfileStore::open_checked(path, || guard.check())
                    })
                    .await?;
                if command_generation != self.generation.load(std::sync::atomic::Ordering::Acquire)
                {
                    return Ok(());
                }
                self.profiles = Some(Arc::new(StdMutex::new(profiles)));
            }
            "profile.check" => {
                let device = self
                    .dlna_discovery
                    .get(string(&c, "deviceId")?)
                    .context("DLNA_UNAVAILABLE")?
                    .clone();
                let profile = live_profile(c["audio"].as_bool().unwrap_or(true));
                let evidence = self
                    .profile_evidence(&device, &profile, command_generation)
                    .await?;
                event(
                    &self.events,
                    "profile.checked",
                    json!({"deviceId":device.id,"profile":profile,"passed":evidence.as_ref().is_some_and(|e| e.passed),"evidence":evidence,"generation":c["generation"],"requestId":c["requestId"]}),
                );
            }
            "probe.confirm" => {
                let context = self.live_context.as_ref().context("NO_PROBE")?;
                let device = &context.device;
                let profile = &context.profile;
                ensure!(context.probe, "NO_PROBE");
                let live = self
                    .live
                    .lock()
                    .unwrap()
                    .as_ref()
                    .cloned()
                    .context("NO_PROBE")?;
                ensure!(!live.cancellation().is_cancelled(), "PROBE_EXPIRED");
                let passed = c["passed"].as_bool().context("PROBE_RESULT_REQUIRED")?;
                ensure!(!passed || live.delivered() > 0, "PROBE_NOT_PULLING");
                let profiles = self.profiles.clone().context("PROFILE_STORE_REQUIRED")?;
                let device_id = device.id.clone();
                let evidence = crate::profiles::Evidence {
                    signature: device.signature.clone(),
                    profile: profile.clone(),
                    confirmed_at: crate::profiles::now(),
                    passed,
                    source: "user_confirmed_synthetic".into(),
                };
                self.io
                    .run(command_generation, move |guard| {
                        let mut profiles = profiles
                            .lock()
                            .map_err(|_| anyhow::anyhow!("PROFILE_STORE_FAILED"))?;
                        guard.check()?;
                        profiles.save_checked(device_id, evidence, || guard.check())
                    })
                    .await?;
                if command_generation != self.generation.load(std::sync::atomic::Ordering::Acquire)
                {
                    return Ok(());
                }
                event(
                    &self.events,
                    "probe.saved",
                    json!({"deviceId":device.id,"passed":passed,"profile":profile}),
                );
                live.revoke();
            }
            "live.create" => {
                ensure!(self.live.lock().unwrap().is_none(), "LIVE_ALREADY_ACTIVE");
                let generation = c["generation"].as_u64().context("GENERATION_REQUIRED")?;
                let device = self
                    .dlna_discovery
                    .get(string(&c, "deviceId")?)
                    .context("DLNA_UNAVAILABLE")?
                    .clone();
                let probe = c["synthetic"].as_bool().unwrap_or(false);
                let profile = live_profile(c["audio"].as_bool().unwrap_or(true));
                if !probe {
                    ensure!(
                        self.profile_evidence(&device, &profile, command_generation)
                            .await?
                            .is_some_and(|e| e.passed),
                        "SYNTHETIC_PROBE_REQUIRED"
                    );
                }
                ensure!(
                    string(&c, "allowedIp")?.parse::<IpAddr>()? == device.ip,
                    "LIVE_DEVICE_MISMATCH"
                );
                let live = crate::live::LiveResource::new(string(&c, "allowedIp")?.parse()?);
                let (url, task) = live.clone().bind(string(&c, "address")?.parse()?).await?;
                *self.live.lock().unwrap() = Some(live);
                self.live_context = Some(LiveContext {
                    device,
                    profile,
                    probe,
                    url: url.clone(),
                    generation,
                });
                self.resource_tasks.push(task);
                event(
                    &self.events,
                    "live.created",
                    json!({"url":url,"mime":"video/mpeg","encrypted":false,"synthetic":probe,"generation":c["generation"]}),
                );
            }
            _ => anyhow::bail!("UNKNOWN_COMMAND"),
        }
        Ok(())
    }
    pub(super) async fn profile_evidence(
        &self,
        device: &dlna::Renderer,
        profile: &str,
        generation: u64,
    ) -> anyhow::Result<Option<crate::profiles::Evidence>> {
        let Some(profiles) = self.profiles.clone() else {
            return Ok(None);
        };
        let device = device.clone();
        let profile = profile.to_owned();
        self.io
            .run(generation, move |guard| {
                let profiles = profiles
                    .lock()
                    .map_err(|_| anyhow::anyhow!("PROFILE_STORE_FAILED"))?;
                guard.check()?;
                Ok(profiles
                    .get(
                        &device.id,
                        &device.signature,
                        &profile,
                        crate::profiles::now(),
                    )
                    .cloned())
            })
            .await
    }
}
fn live_profile(audio: bool) -> String {
    format!(
        "ts-h264-baseline-1280x720-30-v1-{}",
        if audio { "aac48-stereo" } else { "silent" }
    )
}
