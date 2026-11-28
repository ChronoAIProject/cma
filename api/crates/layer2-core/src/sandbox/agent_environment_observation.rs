//! Short-lived capability observations; every request owns its authorization.
use super::*;
use crate::sandbox::agent_environment::{EnvironmentAvailability, EnvironmentFeature};
use moka::sync::Cache;
use sha2::{Digest, Sha256};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::{OnceCell, Semaphore};

#[derive(Default)]
struct Entry {
    started: AtomicBool,
    browser: OnceCell<EnvironmentFeature>,
    control: OnceCell<EnvironmentFeature>,
}
struct Running(Arc<Entry>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.started.store(false, Ordering::Release);
    }
}
pub(in crate::sandbox::agent_filesystem) struct ObservationCache {
    values: Cache<[u8; 32], Arc<Entry>>,
    permits: Arc<Semaphore>,
}
impl ObservationCache {
    pub fn new() -> Self {
        Self {
            values: Cache::builder()
                .max_capacity(256)
                .time_to_live(Duration::from_secs(5))
                .build(),
            permits: Arc::new(Semaphore::new(16)),
        }
    }
}
fn pending() -> EnvironmentFeature {
    EnvironmentFeature {
        state: EnvironmentAvailability::TemporarilyUnavailable,
        reason: "observation_pending",
        scope: "agent_browser",
        writable: false,
    }
}

impl AgentFileService {
    pub(super) async fn environment_features(
        self: &Arc<Self>,
        authorized: Arc<AuthorizedResource>,
        agent_id: &AgentId,
        subject: &crate::SubjectId,
    ) -> Result<(EnvironmentFeature, EnvironmentFeature, bool), AgentFileError> {
        let cache = self
            .environment_cache
            .as_ref()
            .expect("enabled environment observations");
        let sandbox = &authorized.snapshot.lineage.sandbox;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(AgentFileError::NotFound)?;
        if self.adapter.browser().is_none()
            || !authorized.decision.allows(ResourceCapability::ViewHistory)
            || sandbox.status != SandboxStatus::Active
        {
            let browser = self.browser_feature(&authorized, agent_id).await?;
            let control = self
                .browser_control_feature(&authorized, agent_id, &browser)
                .await?;
            self.environment_release(&authorized, agent_id).await?;
            return Ok((browser, control, false));
        }
        let permissions: Vec<_> = authorized.decision.capabilities().collect();
        let authority = (
            &authorized.snapshot.policy,
            &authorized.decision.share_revisions,
            &permissions,
            authorized
                .binding_authority()
                .map(|held| (&held.binding, &held.conversation)),
        );
        let parents = (
            &authorized.snapshot.lineage.workspace,
            &authorized.snapshot.lineage.worktree,
        );
        // Cache neither a credential nor an authorization decision. Scope this
        // advisory observation to current policy, caller and runtime identity.
        let bytes = serde_json::to_vec(&(
            subject,
            agent_id,
            &agent.created_at,
            &sandbox.id,
            &sandbox.created_at,
            &sandbox.owner,
            &sandbox.runtime,
            &sandbox.lifecycle.home,
            &sandbox.status,
            &sandbox.credential,
            authority,
            parents,
        ))
        .map_err(|_| AgentFileError::Unavailable);
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                self.environment_release(&authorized, agent_id).await?;
                return Err(error);
            }
        };
        let key: [u8; 32] = Sha256::digest(bytes).into();
        let cell = cache.values.get_with(key, || Arc::new(Entry::default()));
        let observed_control = cell.control.get().cloned();
        let observed_browser = cell.browser.get().cloned();
        if observed_control.is_none()
            && cell
                .started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let running = Running(cell.clone());
            if let Ok(permit) = cache.permits.clone().try_acquire_owned() {
                let service = self.clone();
                let held = authorized.clone();
                let id = agent_id.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let started = tokio::time::Instant::now();
                    metrics::counter!("cma_agent_environment_observations_total", "outcome" => "started").increment(1);
                    let _running = running;
                    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                    for cap in [
                        ResourceCapability::ViewDetails,
                        ResourceCapability::ViewHistory,
                        ResourceCapability::ManageSharing,
                    ] {
                        if let Some(expires) = held.decision.revalidation_deadline(cap) {
                            deadline = deadline.min(expires);
                        }
                    }
                    let result = tokio::time::timeout_at(deadline, async {
                        // Browser viewing can proceed while control is still being checked.
                        let browser = cell
                            .browser
                            .get_or_try_init(|| service.browser_feature(&held, &id))
                            .await?;
                        let control = service.browser_control_feature(&held, &id, browser).await?;
                        service.environment_release(&held, &id).await?;
                        Ok::<_, AgentFileError>(control)
                    })
                    .await;
                    let outcome = match result {
                        Ok(Ok(value)) => {
                            // The entry owns this generation. Evicted/replaced
                            // entries are never reinserted by a late completion.
                            if cell.control.set(value).is_ok() {
                                "observed"
                            } else {
                                "superseded"
                            }
                        }
                        Ok(Err(_)) => "refused",
                        Err(_) => "timeout",
                    };
                    metrics::counter!("cma_agent_environment_observations_total", "outcome" => outcome).increment(1);
                    metrics::histogram!("cma_agent_environment_observation_seconds", "outcome" => outcome).record(started.elapsed().as_secs_f64());
                    tracing::debug!(outcome, "Agent environment observation finished");
                });
            } else {
                metrics::counter!("cma_agent_environment_observations_total", "outcome" => "capacity").increment(1);
            }
        }
        self.environment_release(&authorized, agent_id).await?;
        let outcome = if observed_control.is_some() {
            "hit"
        } else {
            "pending"
        };
        metrics::counter!("cma_agent_environment_observations_total", "outcome" => outcome)
            .increment(1);
        tracing::debug!(outcome, "Agent environment observation read");
        let browser = observed_browser.unwrap_or_else(pending);
        let control = observed_control.unwrap_or_else(|| {
            if authorized
                .decision
                .allows(ResourceCapability::ManageSharing)
            {
                pending()
            } else {
                EnvironmentFeature {
                    state: EnvironmentAvailability::Forbidden,
                    reason: "browser_owner_required",
                    scope: "agent_browser",
                    writable: false,
                }
            }
        });
        let retry = matches!(
            browser.state,
            EnvironmentAvailability::TemporarilyUnavailable
        ) || matches!(
            control.state,
            EnvironmentAvailability::TemporarilyUnavailable
        );
        Ok((browser, control, retry))
    }

    async fn environment_release(
        &self,
        authorized: &AuthorizedResource,
        id: &AgentId,
    ) -> Result<(), AgentFileError> {
        let current = self
            .access
            .current_read_snapshot(
                authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                ResourceCapability::ViewDetails,
            )
            .await?;
        let before = &authorized.snapshot.lineage;
        if current.lineage.sandbox.runtime != before.sandbox.runtime
            || current.lineage.sandbox.lifecycle.home != before.sandbox.lifecycle.home
            || current.lineage.sandbox.status != before.sandbox.status
            || current.lineage.sandbox.credential != before.sandbox.credential
            || current
                .lineage
                .agent
                .as_ref()
                .map(|agent| (&agent.id, &agent.created_at))
                != before
                    .agent
                    .as_ref()
                    .map(|agent| (&agent.id, &agent.created_at))
        {
            return Err(AgentFileError::Conflict);
        }
        Ok(())
    }
}
