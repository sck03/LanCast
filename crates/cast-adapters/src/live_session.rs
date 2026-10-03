//! DLNA startup and bounded recovery, independent of UI and capture implementation.
use crate::{
    dlna::{Controller, MediaKind},
    live::LiveResource,
};
use anyhow::ensure;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::time::{Instant, sleep};
pub async fn run(
    controller: Controller,
    resource: Arc<LiveResource>,
    url: String,
    probe: bool,
    emit: impl Fn(&str, serde_json::Value),
) -> anyhow::Result<()> {
    let cancel = resource.cancellation();
    let work = async {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !resource.ready() {
            ensure!(Instant::now() < deadline, "LIVE_START_POINT_TIMEOUT");
            sleep(Duration::from_millis(50)).await;
        }
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
        let started = Instant::now();
        let mut deadline = Instant::now() + Duration::from_secs(10);
        let mut rebuilds = 0;
        let mut had_pull = false;
        let mut previous = 0;
        let mut failed_polls = 0;
        loop {
            sleep(Duration::from_secs(1)).await;
            let delivered = resource.delivered();
            if delivered > previous {
                if !had_pull {
                    emit(
                        "live.state",
                        json!({"state":if probe {"awaiting_user_confirmation"} else {"pulling"},"probe":probe,"firstFrameMeasured":false}),
                    );
                }
                had_pull = true;
                deadline = Instant::now() + Duration::from_secs(10);
                previous = delivered;
            }
            if probe {
                ensure!(
                    started.elapsed() < Duration::from_secs(45),
                    "PROBE_CONFIRMATION_TIMEOUT"
                );
            }
            if Instant::now() >= deadline {
                ensure!(had_pull && rebuilds == 0 && !probe, "TV_NOT_PULLING");
                // Do not take over a renderer that has been stopped or switched by its owner.
                ensure!(
                    controller.current_uri().await? == url,
                    "RENDERER_SOURCE_CHANGED"
                );
                ensure!(controller.state().await? == "PLAYING", "RENDERER_STOPPED");
                rebuilds += 1;
                controller
                    .load_media(&url, "LanCast Live", MediaKind::LiveTs)
                    .await?;
                controller.command("play", None).await?;
                deadline = Instant::now() + Duration::from_secs(10);
                emit(
                    "live.state",
                    json!({"state":"recovering","attempt":rebuilds}),
                );
            }
            if started.elapsed().as_secs().is_multiple_of(3) {
                match controller.state().await {
                    Ok(state) => {
                        failed_polls = 0;
                        ensure!(
                            !had_pull || !matches!(state.as_str(), "STOPPED" | "NO_MEDIA_PRESENT"),
                            "RENDERER_STOPPED"
                        );
                    }
                    Err(_) => {
                        failed_polls += 1;
                        ensure!(failed_polls < 3, "RENDERER_UNREACHABLE");
                    }
                }
            }
        }
    };
    let result =
        tokio::select! { biased; _ = cancel.cancelled() => Ok(()), result = work => result };
    resource.revoke();
    // Bounded best-effort cleanup is independent of the cancelled media token.
    let _ = tokio::time::timeout(Duration::from_secs(2), controller.command("stop", None)).await;
    result
}
