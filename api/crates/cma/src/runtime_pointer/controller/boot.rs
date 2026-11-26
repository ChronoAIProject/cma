//! Publish the early writer before configuring or draining any native pool.
use super::*;
use crate::warm_pools::{deployment_image, startup::WarmPoolStartup};

impl RuntimePointerController {
    pub(crate) async fn register_boot(&self) -> Result<(), String> {
        self.release
            .lanes
            .initialize()
            .await
            .map_err(|error| error.to_string())?;
        self.release
            .lanes
            .publish_boot(&self.writer)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) fn spawn_heartbeat(
        self: &Arc<Self>,
        ready: tokio::sync::watch::Receiver<bool>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let controller = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = tick.tick() => {
                        let ready = *ready.borrow();
                        if let Err(error) = controller.release.lanes.renew_writer(&controller.writer, ready).await {
                            tracing::warn!(%error, "runtime pointer writer heartbeat failed");
                        }
                        if let Err(error) = controller.release.refresh().await {
                            tracing::warn!(%error, "durable runtime release refresh failed");
                        }
                    }
                }
            }
            if let Err(error) = controller
                .release
                .lanes
                .remove_writer(&controller.writer)
                .await
            {
                tracing::warn!(%error, "runtime pointer writer lease removal deferred");
            }
        })
    }

    pub(crate) async fn bootstrap(&self) -> Result<(), String> {
        self.release
            .refresh()
            .await
            .map_err(|error| error.to_string())?;
        let captured = self.release.snapshot();
        for (lane, image) in [
            (ReleasedLane::Agent, Some(captured.agent.as_str())),
            (ReleasedLane::Machine, captured.machine.as_deref()),
        ] {
            if let Some(image) = image {
                self.verify_bootstrap_default(lane, image).await?;
            }
        }
        crate::warm_pools::startup::boot_effect(
            Some((&self.release.lanes, &self.writer)),
            self.pools.ensure_default(),
        )
        .await
        .map_err(|error| format!("ensure active native warm pool: {error:?}"))?;
        for (lane, image) in [
            (ReleasedLane::Agent, Some(captured.agent)),
            (ReleasedLane::Machine, captured.machine),
        ] {
            if let Some(image) = image {
                let machine = lane == ReleasedLane::Machine;
                // A machine pointer requires a bootstrapped Agent lane. Its
                // pool can initialize independently while Agent is held.
                if self.pools.default_held(machine) || (machine && self.pools.default_held(false)) {
                    continue;
                }
                if self
                    .release
                    .lanes
                    .load(lane)
                    .await
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    self.verify_bootstrap_default(lane, &image).await?;
                    if self.pools.default_held(machine) {
                        continue;
                    }
                    let record =
                        ReleasedLaneRecord::bootstrap(lane, image, &layer2_core::now_rfc3339())
                            .map_err(|error| error.to_string())?;
                    let target = self
                        .pools
                        .active_default_target(&record.active.image, lane == ReleasedLane::Machine)
                        .await
                        .map_err(|error| format!("locate bootstrap pool: {error:?}"))?;
                    self.targets
                        .retain_release_authority(&target)
                        .await
                        .map_err(|error| format!("retain bootstrap authority: {error:?}"))?;
                    self.release
                        .lanes
                        .bootstrap(record)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        self.release
            .refresh()
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn verify_bootstrap_default(
        &self,
        lane: ReleasedLane,
        image: &str,
    ) -> Result<(), String> {
        let machine = lane == ReleasedLane::Machine;
        let has_release = self
            .release
            .lanes
            .load(lane)
            .await
            .map_err(|error| error.to_string())?
            .is_some();
        let verified = if has_release {
            self.targets.verify_bootstrap(machine, image).await
        } else {
            // Without a pointer record, the snapshot contains a configured
            // pin. Prove any conflict against durable pool order before holding it.
            self.pools
                .hold_conflicting_pin(&deployment_image(image.into()), machine)
                .await
                .map(|_| ())
        };
        verified.map_err(|error| format!("verify {lane:?} runtime release order: {error:?}"))
    }

    pub(crate) async fn await_active_ready(
        &self,
        admission: Option<&crate::startup::Admission>,
    ) -> Result<WarmPoolStartup, String> {
        for _ in 0..3 {
            if admission.is_some_and(crate::startup::Admission::is_draining) {
                return Ok(WarmPoolStartup::Stopping);
            }
            if self.pools.default_held(false) {
                return Ok(WarmPoolStartup::ConfigurationDrift);
            }
            let before = self.release.snapshot();
            let ready = self
                .pools
                .await_default_ready_as(admission, Some((&self.release.lanes, &self.writer)))
                .await;
            let stopping = || admission.is_some_and(crate::startup::Admission::is_draining);
            if stopping() || matches!(&ready, Ok(WarmPoolStartup::Stopping)) {
                return Ok(WarmPoolStartup::Stopping);
            }
            let after = self.release.refresh().await;
            if stopping() {
                return Ok(WarmPoolStartup::Stopping);
            }
            let after = after.map_err(|error| error.to_string())?;
            let same_agent_revision = before.lane_revisions.map(|revisions| revisions.agent)
                == after.lane_revisions.map(|revisions| revisions.agent);
            if before.agent == after.agent
                && (same_agent_revision
                    || !matches!(&ready, Ok(WarmPoolStartup::CapacityDeferred(_))))
            {
                return ready.map_err(|error| {
                    format!("native warm pool startup readiness failed: {error:?}")
                });
            }
            self.bootstrap().await?;
        }
        Err("the active runtime changed repeatedly during startup".into())
    }
}
