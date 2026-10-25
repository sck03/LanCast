//! Bounded disk work. Cancellation prevents queued work and stale commits;
//! an OS call already in progress still owns its resources until it returns.
use anyhow::{Context, ensure};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub(crate) struct BlockingIo {
    slots: Arc<Semaphore>,
    generation: Arc<AtomicU64>,
    shutdown: CancellationToken,
}
pub(crate) struct IoGuard {
    cancelled: CancellationToken,
    generation: Arc<AtomicU64>,
    expected: u64,
    shutdown: CancellationToken,
}
impl IoGuard {
    pub(crate) fn check(&self) -> anyhow::Result<()> {
        ensure!(
            !self.cancelled.is_cancelled()
                && !self.shutdown.is_cancelled()
                && self.generation.load(Ordering::Acquire) == self.expected,
            "COMMAND_CANCELLED"
        );
        Ok(())
    }
}
impl BlockingIo {
    pub(crate) fn new(generation: Arc<AtomicU64>, shutdown: CancellationToken) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(4)),
            generation,
            shutdown,
        }
    }
    pub(crate) async fn run<T, F>(&self, expected: u64, work: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(IoGuard) -> anyhow::Result<T> + Send + 'static,
    {
        let cancelled = CancellationToken::new();
        let _cancel_on_drop = cancelled.clone().drop_guard();
        let guard = IoGuard {
            cancelled,
            generation: self.generation.clone(),
            expected,
            shutdown: self.shutdown.clone(),
        };
        let permit = self.slots.clone().acquire_owned().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            guard.check()?;
            work(guard)
        })
        .await
        .context("DISK_TASK_FAILED")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_disk_work_cannot_commit_after_its_waiter_exits() {
        let io = Arc::new(BlockingIo::new(
            Arc::new(AtomicU64::new(0)),
            CancellationToken::new(),
        ));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let committed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = committed.clone();
        let worker = io.clone();
        let task = tokio::spawn(async move {
            worker
                .run(0, move |guard| {
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    guard.check()?;
                    observed.store(true, Ordering::Release);
                    Ok(())
                })
                .await
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        // Acquiring all permits waits for the old blocking closure to drop its resources.
        let _all = io.slots.acquire_many(4).await.unwrap();
        assert!(!committed.load(Ordering::Acquire));
    }
}
