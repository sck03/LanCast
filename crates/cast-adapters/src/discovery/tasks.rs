//! Search ownership and result delivery; protocol I/O stays in discovery.
use super::{scan_dlna, scan_lancast};
use crate::{
    dlna::Renderer,
    events::{Events, event},
};
use serde_json::json;
use std::{
    collections::HashMap,
    future::{Future, pending},
    net::Ipv4Addr,
};
use tokio::task::JoinHandle;

/// Replacing or stopping a search joins cancellation. Dropping its owner aborts it.
pub(crate) struct ScanTask<T = ()>(Option<JoinHandle<T>>);
impl<T> Default for ScanTask<T> {
    fn default() -> Self {
        Self(None)
    }
}
impl<T: Send + 'static> ScanTask<T> {
    async fn replace(&mut self, work: impl Future<Output = T> + Send + 'static) {
        self.stop().await;
        self.0 = Some(tokio::spawn(work));
    }
    pub async fn stop(&mut self) {
        if let Some(task) = &mut self.0 {
            task.abort();
            let _ = task.await;
        }
        self.0 = None;
    }
}
impl ScanTask {
    pub async fn start(&mut self, events: Events, generation: u64) {
        self.replace(async move {
            let body = match scan_lancast().await {
                Ok(devices) => json!({"devices":devices,"scanGeneration":generation}),
                Err(_) => {
                    json!({"devices":[],"scanGeneration":generation,"error":"DISCOVERY_FAILED"})
                }
            };
            event(&events, "devices", body);
        })
        .await;
    }
}
impl<T> Drop for ScanTask<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

#[derive(Default)]
pub(crate) struct DlnaDiscovery {
    scan: ScanTask<anyhow::Result<Vec<Renderer>>>,
    generation: u64,
    devices: HashMap<String, Renderer>,
}
impl DlnaDiscovery {
    pub async fn start(&mut self, interface: Ipv4Addr, generation: u64) {
        self.replace(generation, scan_dlna(interface)).await;
    }
    async fn replace(
        &mut self,
        generation: u64,
        work: impl Future<Output = anyhow::Result<Vec<Renderer>>> + Send + 'static,
    ) {
        self.scan.replace(work).await;
        self.generation = generation;
        self.devices.clear();
    }
    pub async fn stop(&mut self) {
        self.scan.stop().await;
    }
    pub fn get(&self, id: &str) -> Option<&Renderer> {
        self.devices.get(id)
    }
    /// Cancel-safe until ready. Commit the directory before publishing its event,
    /// on the command owner, so a host can immediately select a reported device.
    pub async fn complete(&mut self, events: &Events) {
        let Some(task) = &mut self.scan.0 else {
            return pending().await;
        };
        let result = task.await;
        self.scan.0 = None;
        let body = match result {
            Ok(Ok(devices)) => {
                let body = json!({"devices":devices,"scanGeneration":self.generation});
                self.devices = devices.into_iter().map(|d| (d.id.clone(), d)).collect();
                body
            }
            _ => json!({"devices":[],"scanGeneration":self.generation,"error":"DISCOVERY_FAILED"}),
        };
        event(events, "dlna.devices", body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    fn renderer(id: &str) -> Renderer {
        Renderer {
            id: id.into(),
            name: id.into(),
            ip: "192.168.1.2".parse().unwrap(),
            transport: crate::dlna::Service {
                kind: "AVTransport:1".into(),
                control: "http://192.168.1.2/control".into(),
            },
            signature: String::new(),
        }
    }

    #[tokio::test]
    async fn pending_completion_can_be_cancelled_without_losing_the_result() {
        let mut discovery = DlnaDiscovery::default();
        let (events, inbox) = crate::events::channel(4, CancellationToken::new());
        let (send, receive) = oneshot::channel();
        discovery
            .replace(17, async { Ok(vec![receive.await.unwrap()]) })
            .await;
        // Other commands can win select! arbitrarily often while this task waits.
        for _ in 0..10 {
            assert!(discovery.complete(&events).now_or_never().is_none());
        }
        send.send(renderer("new")).unwrap();
        discovery.complete(&events).await;
        let result: serde_json::Value = serde_json::from_str(&inbox.try_recv().unwrap()).unwrap();
        assert_eq!(result["body"]["scanGeneration"], 17);
        assert_eq!(result["body"]["devices"][0]["id"], "new");
        assert!(discovery.get("new").is_some());
        assert!(discovery.complete(&events).now_or_never().is_none());
        assert!(inbox.try_recv().is_err());
    }

    #[tokio::test]
    async fn replacement_discards_ready_results_and_clears_the_previous_directory() {
        let mut discovery = DlnaDiscovery::default();
        let (events, inbox) = crate::events::channel(4, CancellationToken::new());
        discovery
            .replace(1, async { Ok(vec![renderer("old")]) })
            .await;
        discovery.complete(&events).await;
        inbox.try_recv().unwrap();
        discovery
            .replace(2, async { Ok(vec![renderer("discarded")]) })
            .await;
        // Wait until the old task has finished but its result has not been delivered.
        while !discovery.scan.0.as_ref().unwrap().is_finished() {
            tokio::task::yield_now().await;
        }
        discovery
            .replace(3, async { Ok(vec![renderer("current")]) })
            .await;
        assert!(discovery.get("old").is_none());
        discovery.complete(&events).await;
        let result: serde_json::Value = serde_json::from_str(&inbox.try_recv().unwrap()).unwrap();
        assert_eq!(result["body"]["scanGeneration"], 3);
        assert_eq!(result["body"]["devices"][0]["id"], "current");
        assert!(discovery.get("discarded").is_none());
        assert!(inbox.try_recv().is_err());
    }

    #[tokio::test]
    async fn stop_joins_pending_discovery_and_discards_undelivered_results() {
        let mut discovery = DlnaDiscovery::default();
        let (events, inbox) = crate::events::channel(4, CancellationToken::new());
        let finished = CancellationToken::new();
        let guard = finished.clone().drop_guard();
        let (started, ready) = oneshot::channel();
        discovery
            .replace(1, async move {
                let _guard = guard;
                started.send(()).unwrap();
                pending().await
            })
            .await;
        ready.await.unwrap();
        discovery.stop().await;
        assert!(finished.is_cancelled());
        discovery
            .replace(2, async { Ok(vec![renderer("undelivered")]) })
            .await;
        while !discovery.scan.0.as_ref().unwrap().is_finished() {
            tokio::task::yield_now().await;
        }
        discovery.stop().await;
        discovery.stop().await;
        assert!(discovery.complete(&events).now_or_never().is_none());
        assert!(discovery.get("undelivered").is_none());
        assert!(inbox.try_recv().is_err());
    }

    #[tokio::test]
    async fn failed_search_is_bounded_and_can_be_replaced() {
        let mut discovery = DlnaDiscovery::default();
        let shutdown = CancellationToken::new();
        let (events, inbox) = crate::events::channel(4, shutdown.clone());
        discovery
            .replace(1, async { anyhow::bail!("x".repeat(256 * 1024)) })
            .await;
        discovery.complete(&events).await;
        let result: serde_json::Value = serde_json::from_str(&inbox.try_recv().unwrap()).unwrap();
        assert_eq!(result["body"]["error"], "DISCOVERY_FAILED");
        assert_eq!(result["body"]["devices"], json!([]));
        assert!(!shutdown.is_cancelled());
        discovery
            .replace(2, async { Ok(vec![renderer("retry")]) })
            .await;
        discovery.complete(&events).await;
        assert!(discovery.get("retry").is_some());
        // Stop keeps an already published selection available for the next cast.
        discovery.stop().await;
        assert!(discovery.get("retry").is_some());
    }

    #[tokio::test]
    async fn interrupted_stop_keeps_ownership_until_the_worker_exits() {
        let mut scan = ScanTask::default();
        let finished = CancellationToken::new();
        let guard = finished.clone().drop_guard();
        let (started, ready) = oneshot::channel();
        scan.replace(async move {
            let _guard = guard;
            started.send(()).unwrap();
            pending::<()>().await;
        })
        .await;
        ready.await.unwrap();
        assert!(scan.stop().now_or_never().is_none());
        assert!(scan.0.is_some());
        scan.stop().await;
        assert!(finished.is_cancelled());
        assert!(scan.0.is_none());
    }

    #[tokio::test]
    async fn replacement_joins_old_scan_and_stop_is_idempotent() {
        let mut scan = ScanTask::default();
        let mut previous: Option<CancellationToken> = None;
        for _ in 0..8 {
            let finished = CancellationToken::new();
            let guard = finished.clone().drop_guard();
            let (started, ready) = oneshot::channel();
            let old = previous.clone();
            scan.replace(async move {
                let _guard = guard;
                assert!(old.is_none_or(|old| old.is_cancelled()));
                started.send(()).unwrap();
                pending::<()>().await;
            })
            .await;
            ready.await.unwrap();
            previous = Some(finished);
        }
        scan.stop().await;
        assert!(previous.unwrap().is_cancelled());
        scan.stop().await;
    }

    #[tokio::test]
    async fn dropping_scan_owner_cancels_pending_work() {
        let mut scan = ScanTask::default();
        let finished = CancellationToken::new();
        let guard = finished.clone().drop_guard();
        let (started, ready) = oneshot::channel();
        scan.replace(async move {
            let _guard = guard;
            started.send(()).unwrap();
            pending::<()>().await;
        })
        .await;
        ready.await.unwrap();
        drop(scan);
        tokio::time::timeout(Duration::from_secs(1), finished.cancelled())
            .await
            .unwrap();
    }
}
