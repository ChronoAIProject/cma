//! The resolver observes and verifies registry state. Mongo decides what can serve.
use std::sync::Arc;
use std::time::Duration;

use layer2_core::released_runtime::{CandidateState, PromotionEvidence, ReleasedLaneRecord};
use layer2_core::sandbox::SandboxStoreError;
use layer3_adapter::ReleasedRuntimeSource;
use mongodb::bson::DateTime;

use super::*;
use crate::store::native_warm_pools::MongoNativeWarmPools;
use crate::store::released_runtimes::{BootWriter, ResolverLease, StoreReleasedRuntime};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::warm_pools::NativeWarmPools;

mod boot;

const CANDIDATE_DEADLINE_SECS: u64 = 15 * 60;

pub(crate) struct RuntimePointerController {
    pub(crate) release: Arc<StoreReleasedRuntime>,
    targets: Arc<MongoNativeWarmPools>,
    pools: Arc<NativeWarmPools>,
    pointer: Option<Arc<dyn RuntimePointerSource>>,
    writer: BootWriter,
    cutover_epoch: Option<uuid::Uuid>,
}

impl RuntimePointerController {
    pub(crate) fn compose(
        config: &crate::config::Config,
        stores: &crate::store::MongoStores,
        pools: Arc<NativeWarmPools>,
    ) -> Result<Option<Arc<Self>>, String> {
        let Some(release) = &stores.runtime_pointer else {
            return Ok(None);
        };
        let pointer = config
            .runtime
            .runtime_pointer
            .as_ref()
            .filter(|config| config.can_follow())
            .cloned()
            .map(RegistryRuntimePointers::new)
            .transpose()
            .map_err(|error| error.to_string())?
            .map(|source| Arc::new(source) as Arc<dyn RuntimePointerSource>);
        let incarnation = stores
            .sandbox_model
            .agent_manager_instance()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .ok_or("runtime pointer requires Agent Manager replica ownership")?;
        let pod_uid = std::env::var("CHRONO_AM_POD_UID")
            .ok()
            .and_then(|id| uuid::Uuid::parse_str(&id).ok())
            .ok_or("runtime pointer requires CHRONO_AM_POD_UID")?;
        let build_digest = std::env::var("CHRONO_AM_BUILD_DIGEST")
            .ok()
            .filter(|digest| super::registry::valid_digest(digest))
            .ok_or("runtime pointer requires a sha256 CHRONO_AM_BUILD_DIGEST")?;
        let cutover_epoch = config
            .runtime
            .runtime_pointer
            .as_ref()
            .and_then(|config| config.cutover_epoch());
        Ok(Some(Arc::new(Self::new(
            release.clone(),
            stores.native_warm_pools.clone(),
            pools,
            pointer,
            BootWriter {
                incarnation,
                pod_uid,
                build_digest,
            },
            cutover_epoch,
        ))))
    }

    pub(crate) fn following_configured(&self) -> bool {
        self.pointer.is_some()
    }

    pub(crate) fn new(
        release: Arc<StoreReleasedRuntime>,
        targets: Arc<MongoNativeWarmPools>,
        pools: Arc<NativeWarmPools>,
        pointer: Option<Arc<dyn RuntimePointerSource>>,
        writer: BootWriter,
        cutover_epoch: Option<uuid::Uuid>,
    ) -> Self {
        Self {
            release,
            targets,
            pools,
            pointer,
            writer,
            cutover_epoch,
        }
    }

    pub(crate) fn spawn(
        self: Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = tick.tick() => {
                        for lane in [ReleasedLane::Agent, ReleasedLane::Machine] {
                            if let Err(error) = self.reconcile_once(lane).await {
                                match error {
                                    SandboxStoreError::Conflict => tracing::debug!(?lane, "runtime pointer decision deferred"),
                                    _ => tracing::warn!(?lane, %error, "runtime pointer reconciliation failed"),
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    pub(crate) async fn set_following(&self, follow: bool) -> Result<(), SandboxStoreError> {
        super::operator::set_following(&self.release.lanes, &self.targets, follow).await
    }

    pub(crate) async fn reconcile_once(&self, lane: ReleasedLane) -> Result<(), SandboxStoreError> {
        let Some(pointer) = &self.pointer else {
            return Ok(());
        };
        let Some(current) = self.release.lanes.load(lane).await? else {
            return Ok(());
        };
        if current.frozen {
            return Ok(());
        }
        let Some(lease) = self.release.lanes.elect(&self.writer, lane).await? else {
            return Ok(());
        };
        let Some(observation) = pointer.observe(lane).await.map_err(unavailable)? else {
            return Ok(());
        };
        let (image, refusal) = match observation {
            PointerObservation::Verified(image) => (image, None),
            PointerObservation::Refused { image, reason } => (image, Some(reason)),
        };
        if let Some(candidate) = current.candidate.as_ref().filter(|candidate| {
            candidate.image == image && matches!(candidate.state, CandidateState::Refused { .. })
        }) {
            if self
                .targets
                .prepared_candidate(lane == ReleasedLane::Machine, candidate)
                .await
                .map_err(pool_error)?
                .is_some()
            {
                let mut session = self.release.lanes.session().await?;
                self.release
                    .lanes
                    .fence_current(&current, &mut session)
                    .await?;
                self.targets
                    .clear_candidate(lane == ReleasedLane::Machine, Some(candidate), &mut session)
                    .await
                    .map_err(pool_error)?;
                session.commit_guarded().await.map_err(unavailable)?;
            }
            return Ok(());
        }
        if image == current.active.image && current.candidate.is_none() {
            return Ok(());
        }
        let machine = lane == ReleasedLane::Machine;
        if image == current.active.image
            || current
                .candidate
                .as_ref()
                .is_none_or(|candidate| candidate.image != image)
            || refusal.is_some()
        {
            return self
                .record_observation(&current, &lease, image, refusal)
                .await;
        }
        let observed = current
            .candidate
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let active = self
            .pools
            .active_default_target(&current.active.image, machine)
            .await
            .map_err(pool_error)?;
        let Some(candidate) = self
            .targets
            .prepared_candidate(machine, observed)
            .await
            .map_err(pool_error)?
        else {
            return Err(SandboxStoreError::Conflict);
        };
        if candidate.target_replicas != active.target_replicas {
            let mut session = self.release.lanes.session().await?;
            self.release
                .lanes
                .fence_writers(&lease, &mut session)
                .await?;
            self.targets
                .resize_candidate(observed, &candidate, &active, &mut session)
                .await
                .map_err(pool_error)?;
            if candidate.target_replicas == 0 {
                let mut next = current
                    .settle(&observed.observation_id, CandidateState::Preparing)
                    .map_err(|_| SandboxStoreError::Conflict)?;
                next.candidate
                    .as_mut()
                    .ok_or(SandboxStoreError::Conflict)?
                    .since = layer2_core::now_rfc3339();
                self.release
                    .lanes
                    .replace(&current, next, Some(&mut session))
                    .await?;
            } else {
                self.release
                    .lanes
                    .fence_current(&current, &mut session)
                    .await?;
            }
            return session.commit_guarded().await.map_err(unavailable);
        }
        if active.target_replicas == 0 {
            return Ok(());
        }
        let now = layer2_core::now_rfc3339();
        let elapsed = layer2_core::seconds_from_rfc3339(&now)
            .zip(layer2_core::seconds_from_rfc3339(&observed.since))
            .map(|(now, since)| now.saturating_sub(since))
            .ok_or(SandboxStoreError::Conflict)?;
        if elapsed >= CANDIDATE_DEADLINE_SECS {
            return self
                .record_observation(&current, &lease, image, Some("not_ready"))
                .await;
        }
        let Some(ready) = self
            .pools
            .candidate_inventory(&candidate)
            .await
            .map_err(pool_error)?
        else {
            return Ok(());
        };
        if pointer.observe(lane).await.map_err(unavailable)?
            != Some(PointerObservation::Verified(image))
        {
            return Ok(());
        }

        let mut session = self.release.lanes.session().await?;
        let now_ms = self
            .release
            .lanes
            .fence_writers(&lease, &mut session)
            .await?;
        if lane == ReleasedLane::Agent
            && self
                .release
                .lanes
                .unresolved_legacy_dispatches(&mut session)
                .await?
        {
            return Err(SandboxStoreError::Conflict);
        }
        let ready_lane = current
            .settle(&observed.observation_id, CandidateState::Ready)
            .map_err(|_| SandboxStoreError::Conflict)?;
        let (next, transition) = ready_lane
            .promote(
                &PromotionEvidence {
                    image: observed.image.clone(),
                    observation_id: observed.observation_id.clone(),
                    lane_revision: ready_lane.revision,
                },
                &now,
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        self.targets
            .promote_candidate(
                &current,
                &active,
                &candidate,
                &ready,
                DateTime::from_millis(now_ms),
                &mut session,
            )
            .await
            .map_err(pool_error)?;
        self.release
            .lanes
            .record_promotion(&current, next, transition.clone(), &mut session)
            .await?;
        self.release
            .lanes
            .finish_promotion(&mut session, &transition)
            .await?;
        tracing::info!(?lane, previous = %transition.from_image, active = %transition.to_image, "released runtime promoted");
        self.release.refresh().await?;
        Ok(())
    }

    async fn record_observation(
        &self,
        current: &ReleasedLaneRecord,
        lease: &ResolverLease,
        image: String,
        refusal: Option<&'static str>,
    ) -> Result<(), SandboxStoreError> {
        let lane = current.lane;
        let machine = lane == ReleasedLane::Machine;
        let mut next = current
            .observe(
                image,
                uuid::Uuid::new_v4().to_string(),
                &layer2_core::now_rfc3339(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        let mut session = self.release.lanes.session().await?;
        self.release
            .lanes
            .fence_writers(lease, &mut session)
            .await?;
        if lane == ReleasedLane::Agent
            && next.candidate.is_some()
            && refusal.is_none()
            && self
                .release
                .lanes
                .unresolved_legacy_dispatches(&mut session)
                .await?
        {
            return Err(SandboxStoreError::Conflict);
        }
        let refusal = match (&next.candidate, refusal) {
            (Some(candidate), None)
                if self
                    .targets
                    .historical_candidate(machine, &candidate.image, &mut session)
                    .await
                    .map_err(pool_error)? =>
            {
                Some("historical_target")
            }
            _ => refusal,
        };
        if let Some(previous) = &current.candidate {
            match self
                .targets
                .clear_candidate(machine, Some(previous), &mut session)
                .await
            {
                Ok(()) => {}
                Err(layer1_public::rest::admin::warm_pools::WarmPoolError::Conflict)
                    if matches!(previous.state, CandidateState::Preparing) =>
                {
                    // Close admissions before waiting for the old target's
                    // provider work. A busy candidate must not keep extending
                    // the effect bound after its pointer moves or times out.
                    let reason = if next
                        .candidate
                        .as_ref()
                        .is_some_and(|candidate| candidate.image == previous.image)
                    {
                        refusal.unwrap_or("superseded")
                    } else {
                        "superseded"
                    };
                    let quiescing = current
                        .settle(
                            &previous.observation_id,
                            CandidateState::Refused {
                                reason: reason.into(),
                            },
                        )
                        .map_err(|_| SandboxStoreError::Conflict)?;
                    self.release
                        .lanes
                        .replace(current, quiescing, Some(&mut session))
                        .await?;
                    return session.commit_guarded().await.map_err(unavailable);
                }
                Err(error) => return Err(pool_error(error)),
            }
        }
        match (&next.candidate, refusal) {
            (Some(candidate), Some(reason)) => {
                next = next
                    .settle(
                        &candidate.observation_id,
                        CandidateState::Refused {
                            reason: reason.into(),
                        },
                    )
                    .map_err(|_| SandboxStoreError::Conflict)?;
            }
            (Some(candidate), None) => {
                let active = self
                    .pools
                    .active_default_target(&current.active.image, machine)
                    .await
                    .map_err(pool_error)?;
                let target = self
                    .pools
                    .candidate_target(&candidate.image, &active)
                    .map_err(pool_error)?;
                self.targets
                    .prepare_candidate(target, candidate, &mut session)
                    .await
                    .map_err(pool_error)?;
            }
            (None, _) => {}
        }
        let candidate = next
            .candidate
            .as_ref()
            .map(|candidate| candidate.image.clone());
        self.release
            .lanes
            .replace(current, next, Some(&mut session))
            .await?;
        session.commit_guarded().await.map_err(unavailable)?;
        tracing::info!(
            ?lane,
            ?candidate,
            ?refusal,
            "released runtime pointer observed"
        );
        Ok(())
    }
}

fn unavailable(_: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable("runtime pointer decision could not be completed".into())
}

fn pool_error(error: layer1_public::rest::admin::warm_pools::WarmPoolError) -> SandboxStoreError {
    if matches!(
        error,
        layer1_public::rest::admin::warm_pools::WarmPoolError::Conflict
    ) {
        SandboxStoreError::Conflict
    } else {
        unavailable(format!("{error:?}"))
    }
}
