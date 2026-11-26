//! Publish the early writer before configuring or draining any native pool.
use super::*;
use crate::warm_pools::{deployment_image, startup::WarmPoolStartup};
use layer1_public::rest::admin::warm_pools::WarmPoolError;

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
        let captured = self
            .release
            .refresh()
            .await
            .map_err(|error| error.to_string())?;
        for (lane, pin) in [
            (ReleasedLane::Agent, Some(captured.agent.as_str())),
            (ReleasedLane::Machine, captured.machine.as_deref()),
        ] {
            if let Some(pin) = pin {
                self.bootstrap_lane(lane, pin).await?;
            }
        }
        self.release
            .refresh()
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn bootstrap_lane(&self, lane: ReleasedLane, pin: &str) -> Result<(), String> {
        let machine = lane == ReleasedLane::Machine;
        let writer = Some((&self.release.lanes, &self.writer));
        for _ in 0..3 {
            let current = self
                .release
                .lanes
                .load(lane)
                .await
                .map_err(|error| error.to_string())?;
            let image = current
                .as_ref()
                .map_or(pin, |record| record.active.image.as_str());
            let can_publish = current.is_none()
                && (lane == ReleasedLane::Agent
                    || self
                        .release
                        .lanes
                        .load(ReleasedLane::Agent)
                        .await
                        .map_err(|error| error.to_string())?
                        .is_some());
            let pool = crate::warm_pools::startup::boot_effect(writer, async {
                if self.pools.default_held(machine) {
                    return Ok(Some(()));
                }
                // Only errors raised inside this effect can be pool conflicts.
                // A failed boot-writer guard still propagates as an error.
                match self
                    .ensure_bootstrap_pool(image, machine, can_publish)
                    .await
                {
                    Err(WarmPoolError::Conflict) => Ok(None),
                    result => result.map(Some),
                }
            })
            .await
            .map_err(|error| format!("configure {lane:?} startup pool: {error:?}"))?;
            if pool.is_some() {
                if can_publish && !self.pools.default_held(machine) {
                    self.publish_bootstrap(lane, image).await?;
                }
                return Ok(());
            }

            let observed = self
                .release
                .lanes
                .load(lane)
                .await
                .map_err(|error| error.to_string())?;
            if observed != current {
                continue;
            }
            if observed.is_some() {
                if machine
                    && crate::warm_pools::startup::boot_effect(writer, async {
                        // A released image can have a newer exact Desktop
                        // sizing plan. Only hold the lane after confirming
                        // that the durable image itself still agrees.
                        self.targets.verify_bootstrap(true, image).await?;
                        self.pools
                            .hold_conflicting_pin(&deployment_image(image.into()), true)
                            .await
                    })
                    .await
                    .map_err(|error| format!("verify {lane:?} released pool plan: {error:?}"))?
                {
                    return Ok(());
                }
                return Err(format!(
                    "durable {lane:?} pointer conflicts with pool authority"
                ));
            }
            if crate::warm_pools::startup::boot_effect(
                writer,
                self.pools
                    .hold_conflicting_pin(&deployment_image(pin.into()), machine),
            )
            .await
            .map_err(|error| format!("verify {lane:?} fixed runtime pin: {error:?}"))?
            {
                return Ok(());
            }
        }
        Err(format!(
            "{lane:?} pool authority changed repeatedly during startup"
        ))
    }

    async fn ensure_bootstrap_pool(
        &self,
        image: &str,
        machine: bool,
        can_publish: bool,
    ) -> Result<(), WarmPoolError> {
        self.targets.verify_bootstrap(machine, image).await?;
        self.pools.ensure_released_image(image, machine).await?;
        if can_publish {
            self.targets.verify_bootstrap(machine, image).await?;
            let target = self.pools.active_default_target(image, machine).await?;
            self.targets.retain_release_authority(&target).await?;
        }
        Ok(())
    }

    async fn publish_bootstrap(&self, lane: ReleasedLane, image: &str) -> Result<(), String> {
        let record = ReleasedLaneRecord::bootstrap(lane, image.into(), &layer2_core::now_rfc3339())
            .map_err(|error| error.to_string())?;
        self.release
            .lanes
            .bootstrap(record)
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
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
