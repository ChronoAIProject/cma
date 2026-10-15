//! Restartable execution of an owner's approved Agent scope request.

use std::{sync::Arc, time::Duration};

use layer2_core::resource_access::CallerContext;
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::agent_io::{AgentCommandService, CommandAuthorizationError};
use layer2_core::sandbox::SandboxStatus;
use layer2_core::trigger_domain::{ServiceConnectionBroker, ServiceRequirementState};
use tokio::{sync::watch, task::JoinHandle};

use crate::auth::{persistence::SecretValue, AuthState};
use crate::store::agent_access_requests::{
    AgentAccessDecision, AgentAccessPhase as Phase, AgentAccessRecord,
};
use crate::store::MongoSandboxStore;

mod effects;
mod notification;
mod waiting_quiet;

pub(crate) struct AgentAccessWorker {
    store: Arc<MongoSandboxStore>,
    auth: Arc<AuthState>,
    broker: Arc<dyn ServiceConnectionBroker>,
    commands: Arc<AgentCommandService>,
    holder: String,
}

impl AgentAccessWorker {
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        auth: Arc<AuthState>,
        broker: Arc<dyn ServiceConnectionBroker>,
        commands: Arc<AgentCommandService>,
        holder: String,
    ) -> Self {
        Self {
            store,
            auth,
            broker,
            commands,
            holder,
        }
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                    _ = interval.tick() => {
                        if *shutdown.borrow() { break; }
                        // Each iteration performs one bounded durable phase, so
                        // a pending owner connection never holds the worker.
                        if self.tick().await.is_err() {
                            tracing::warn!("Agent access workflow will retry a durable request");
                        }
                    }
                }
            }
        })
    }

    async fn tick(&self) -> Result<(), AgentAccessError> {
        let Some(record) = self.store.claim_access_request(&self.holder).await? else {
            return Ok(());
        };
        let lease = record.lease.as_ref().ok_or(AgentAccessError::Conflict)?;
        // Every effect has its own finite timeout; renew while waiting on IO.
        // Lost leases stop this attempt without borrowing another claim.
        let result = {
            let work = tokio::time::timeout(Duration::from_secs(60), self.step(&record));
            tokio::pin!(work);
            let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
            heartbeat.tick().await;
            loop {
                tokio::select! {
                    result = &mut work => break result.unwrap_or(Err(AgentAccessError::Unavailable)),
                    _ = heartbeat.tick() => {
                        self.store.renew_access_lease(lease).await?;
                    }
                }
            }
        };
        match result {
            Ok(()) | Err(AgentAccessError::Conflict | AgentAccessError::Unavailable) => {
                // Phase changes preserve lease authority; final phases clear it.
                match self
                    .store
                    .release_access_request(lease, Duration::from_secs(2))
                    .await
                {
                    Ok(()) | Err(AgentAccessError::Conflict) => Ok(()),
                    Err(error) => Err(error),
                }
            }
            Err(AgentAccessError::Forbidden) => {
                self.store
                    .fail_access_request(lease, "owner_authority_revoked")
                    .await?;
                Ok(())
            }
            Err(AgentAccessError::NotFound) => {
                self.store
                    .fail_access_request(lease, "agent_identity_changed")
                    .await?;
                Ok(())
            }
            Err(AgentAccessError::Invalid) => {
                self.store
                    .fail_access_request(lease, "access_update_cannot_continue")
                    .await?;
                Ok(())
            }
        }
    }

    async fn authority(
        &self,
        record: &AgentAccessRecord,
    ) -> Result<(CallerContext, SecretValue), AgentAccessError> {
        let Some(AgentAccessDecision::Approved { authorization, .. }) = &record.decision else {
            return Err(AgentAccessError::Forbidden);
        };
        self.auth
            .retained_personal_owner_authority(&record.requester.owner, authorization)
            .await
            .map_err(|error| match error {
                CommandAuthorizationError::Unavailable => AgentAccessError::Unavailable,
                _ => AgentAccessError::Forbidden,
            })
    }

    async fn connections_ready(
        &self,
        record: &AgentAccessRecord,
    ) -> Result<bool, AgentAccessError> {
        self.authority(record).await?;
        let services = record.approved_services()?;
        let requirements = tokio::time::timeout(
            Duration::from_secs(20),
            self.broker.requirements(&record.requester.owner, services),
        )
        .await
        .map_err(|_| AgentAccessError::Unavailable)?
        .map_err(|_| AgentAccessError::Unavailable)?;
        // Validate the full set, including duplicates/omissions; a hosted popup
        // callback or a successful browser close is never connection evidence.
        let ready = requirements.len() == services.len()
            && services.iter().all(|service| {
                requirements
                    .iter()
                    .filter(|row| &row.service == service)
                    .count()
                    == 1
                    && requirements.iter().any(|row| {
                        &row.service == service && row.state == ServiceRequirementState::Satisfied
                    })
            });
        self.authority(record).await?;
        Ok(ready)
    }

    async fn step(&self, record: &AgentAccessRecord) -> Result<(), AgentAccessError> {
        let lease = record.lease.as_ref().ok_or(AgentAccessError::Conflict)?;
        let age = layer2_core::seconds_from_rfc3339(&record.created_at)
            .and_then(|created| {
                layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
                    .map(|now| now.saturating_sub(created))
            })
            .ok_or(AgentAccessError::Invalid)?;
        if matches!(
            record.phase,
            Phase::Pending | Phase::Connecting | Phase::WaitingQuiet
        ) && age >= 86400
        {
            self.store
                .fail_access_request(lease, "access_request_expired")
                .await?;
            return Ok(());
        }
        if record.phase == Phase::Pending {
            return Ok(());
        }
        let (context, credential) = self.authority(record).await?;
        let (agent, sandbox) = self.store.verify_access_request_lineage(record).await?;
        match record.phase {
            Phase::Connecting => {
                if self.connections_ready(record).await? {
                    self.advance(record, Phase::WaitingQuiet).await?;
                }
            }
            Phase::WaitingQuiet => {
                waiting_quiet::schedule(
                    &self.store,
                    record,
                    &sandbox,
                    self.connections_ready(record),
                )
                .await?;
            }
            Phase::Pausing => self.paused(record, &sandbox).await?,
            Phase::Granting => self.grant(record, &sandbox).await?,
            Phase::Reviving => self.revive(record, &sandbox).await?,
            Phase::Notifying => {
                self.notify(record, &context, credential.expose(), &agent, &sandbox)
                    .await?
            }
            Phase::Pending | Phase::Succeeded | Phase::Denied | Phase::Failed => {}
        }
        Ok(())
    }

    async fn advance(
        &self,
        record: &AgentAccessRecord,
        next: Phase,
    ) -> Result<(), AgentAccessError> {
        self.store
            .advance_access_request(
                record.lease.as_ref().ok_or(AgentAccessError::Conflict)?,
                record.phase,
                next,
                record.progress.clone(),
            )
            .await?;
        Ok(())
    }
}
