//! A receiver's loopback file resource shares its engine, but owns its cancellation.
use crate::legacy_bridge::Bridge;
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;

#[derive(Default)]
pub(super) struct BridgeResource {
    active: Option<(String, Arc<Bridge>, JoinHandle<()>)>,
}

impl BridgeResource {
    pub(super) async fn open(
        &mut self,
        id: String,
        source: &str,
        pin: &str,
    ) -> anyhow::Result<String> {
        self.close(None).await;
        let (url, bridge, task) = Bridge::bind(source, pin).await?;
        self.active = Some((id, bridge, task));
        Ok(url)
    }

    pub(super) async fn close(&mut self, id: Option<&str>) {
        if id.is_some_and(|id| self.active.as_ref().is_none_or(|entry| entry.0 != id)) {
            return;
        }
        if let Some((_, bridge, task)) = self.active.as_mut() {
            bridge.revoke();
            if tokio::time::timeout(Duration::from_secs(2), &mut *task)
                .await
                .is_err()
            {
                task.abort();
                let _ = (&mut *task).await;
            }
        }
        self.active = None;
    }
}

impl Drop for BridgeResource {
    fn drop(&mut self) {
        if let Some((_, bridge, task)) = &self.active {
            bridge.revoke();
            task.abort();
        }
    }
}
