//! DLNA startup and bounded recovery, independent of UI and capture implementation.
use crate::{
    dlna::{Controller, MediaKind},
    live::LiveResource,
};
use anyhow::ensure;
use serde_json::json;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::{Instant, sleep};

const PULL_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);

pub async fn run(
    controller: Controller,
    resource: Arc<LiveResource>,
    url: String,
    probe: bool,
    emit: impl Fn(&str, serde_json::Value),
) -> anyhow::Result<()> {
    let cancel = resource.cancellation();
    let work = async {
        resource.wait_ready().await?;
        // A timed-out mutation is not blindly repeated. Query the current URI before Play.
        if let Err(error) = controller
            .load_media(&url, "LanCast Live", MediaKind::LiveTs)
            .await
            && controller.current_uri().await? != url
        {
            return Err(error);
        }
        if let Err(error) = controller.command("play", None).await
            && controller.state().await? != "PLAYING"
        {
            return Err(error);
        }
        emit(
            "live.state",
            json!({"state":"confirming_pull","probe":probe,"firstFrameMeasured":false}),
        );
        let mut previous = 0;
        let mut rebuilt = false;
        loop {
            let had_pull = monitor(
                || resource.delivered(),
                || controller.state(),
                previous,
                probe,
                || emit(
                    "live.state",
                    json!({"state":if probe {"awaiting_user_confirmation"} else {"pulling"},"probe":probe,"firstFrameMeasured":false}),
                ),
            ).await?;
            ensure!(had_pull && !rebuilt && !probe, "TV_NOT_PULLING");
            // Do not take over a renderer that has been stopped or switched by its owner.
            ensure!(
                controller.current_uri().await? == url,
                "RENDERER_SOURCE_CHANGED"
            );
            ensure!(controller.state().await? == "PLAYING", "RENDERER_STOPPED");
            // Ignore bytes from the old pull, but retain progress during the rebuild.
            previous = resource.delivered();
            controller
                .load_media(&url, "LanCast Live", MediaKind::LiveTs)
                .await?;
            controller.command("play", None).await?;
            rebuilt = true;
            emit("live.state", json!({"state":"recovering","attempt":1}));
        }
    };
    let result =
        tokio::select! { biased; _ = cancel.cancelled() => Ok(()), result = work => result };
    resource.revoke();
    // Bounded best-effort cleanup is independent of the cancelled media token.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        if controller.current_uri().await? == url {
            controller.command("stop", None).await?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    result
}

// These futures remain owned by the session: no detached tasks, overlapping
// queries, or background polling survives a stall, deadline, or cancellation.
async fn monitor<F: Future<Output = anyhow::Result<String>>>(
    delivered: impl Fn() -> u64,
    mut state: impl FnMut() -> F,
    mut previous: u64,
    probe: bool,
    on_pull: impl Fn(),
) -> anyhow::Result<bool> {
    let pull = async {
        let mut deadline = Instant::now() + PULL_TIMEOUT;
        let mut had_pull = false;
        loop {
            sleep(Duration::from_secs(1)).await;
            if Instant::now() >= deadline {
                return had_pull;
            }
            let current = delivered();
            if current > previous {
                if !had_pull {
                    on_pull();
                }
                had_pull = true;
                previous = current;
                deadline = Instant::now() + PULL_TIMEOUT;
            }
        }
    };
    let transport = async {
        let mut failures = 0;
        loop {
            // Schedule from completion. Slow responses cannot cause missed
            // modulo ticks or a burst of catch-up queries.
            sleep(POLL_INTERVAL).await;
            match state().await {
                Ok(state) => {
                    failures = 0;
                    ensure!(
                        delivered() == 0
                            || !matches!(state.as_str(), "STOPPED" | "NO_MEDIA_PRESENT"),
                        "RENDERER_STOPPED"
                    );
                }
                Err(_) => {
                    failures += 1;
                    ensure!(failures < 3, "RENDERER_UNREACHABLE");
                }
            }
        }
    };
    tokio::select! { biased;
        _ = sleep(PROBE_TIMEOUT), if probe => anyhow::bail!("PROBE_CONFIRMATION_TIMEOUT"),
        result = transport => result,
        had_pull = pull => Ok(had_pull),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        future::pending,
    };
    use tokio_util::sync::CancellationToken;

    #[tokio::test(start_paused = true)]
    async fn stalled_query_cannot_delay_pull_deadline_and_is_dropped() {
        let dropped = CancellationToken::new();
        let started = Instant::now();
        let result = monitor(
            || 0,
            || async {
                let _drop = dropped.clone().drop_guard();
                pending().await
            },
            0,
            false,
            || panic!("no media delivered"),
        )
        .await
        .unwrap();
        assert!(!result);
        assert_eq!(started.elapsed(), PULL_TIMEOUT);
        assert!(dropped.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn probe_deadline_wins_over_slow_queries_and_continuous_media() {
        let started = Instant::now();
        let dropped = CancellationToken::new();
        let pulls = Cell::new(0);
        let result = monitor(
            || started.elapsed().as_secs(),
            || async {
                let _drop = dropped.clone().drop_guard();
                sleep(Duration::from_secs(100)).await;
                Ok("PLAYING".into())
            },
            0,
            true,
            || pulls.set(pulls.get() + 1),
        )
        .await
        .unwrap_err();
        assert_eq!(result.to_string(), "PROBE_CONFIRMATION_TIMEOUT");
        assert_eq!(started.elapsed(), PROBE_TIMEOUT);
        assert_eq!(pulls.get(), 1);
        assert!(dropped.is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn slow_queries_keep_their_cadence_and_success_resets_failures() {
        let started = Instant::now();
        let queries = RefCell::new(Vec::new());
        let result = monitor(
            || started.elapsed().as_secs(),
            || async {
                queries.borrow_mut().push(started.elapsed().as_secs());
                sleep(Duration::from_secs(2)).await;
                if queries.borrow().len() == 3 {
                    Ok("PLAYING".into())
                } else {
                    anyhow::bail!("unreachable")
                }
            },
            0,
            false,
            || {},
        )
        .await
        .unwrap_err();
        assert_eq!(result.to_string(), "RENDERER_UNREACHABLE");
        assert_eq!(*queries.borrow(), [3, 8, 13, 18, 23, 28]);
        assert_eq!(started.elapsed(), Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_requires_new_media_and_reports_resumed_pull() {
        let pulls = Cell::new(0);
        assert!(
            !monitor(|| 100, pending, 100, false, || pulls.set(pulls.get() + 1))
                .await
                .unwrap()
        );
        assert_eq!(pulls.get(), 0);
        assert!(
            monitor(|| 101, pending, 100, false, || pulls.set(pulls.get() + 1))
                .await
                .unwrap()
        );
        assert_eq!(pulls.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn expired_pull_cannot_be_revived_by_late_media() {
        let delivered = Cell::new(0);
        let monitor = monitor(
            || delivered.get(),
            pending,
            0,
            false,
            || panic!("late media accepted"),
        );
        tokio::pin!(monitor);
        assert!(futures_util::poll!(monitor.as_mut()).is_pending());
        tokio::time::advance(PULL_TIMEOUT).await;
        delivered.set(1);
        assert!(!monitor.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn renderer_stop_and_session_cancellation_release_pending_queries() {
        for state in ["STOPPED", "NO_MEDIA_PRESENT"] {
            let error = monitor(
                || 1,
                || std::future::ready(Ok(state.into())),
                0,
                false,
                || {},
            )
            .await
            .unwrap_err();
            assert_eq!(error.to_string(), "RENDERER_STOPPED");
        }
        let dropped = CancellationToken::new();
        let future = monitor(
            || 1,
            || async {
                let _drop = dropped.clone().drop_guard();
                pending().await
            },
            0,
            false,
            || {},
        );
        let mut future = Box::pin(future);
        assert!(futures_util::poll!(&mut future).is_pending());
        tokio::time::advance(POLL_INTERVAL).await;
        assert!(futures_util::poll!(&mut future).is_pending());
        drop(future);
        assert!(dropped.is_cancelled());
    }
}
