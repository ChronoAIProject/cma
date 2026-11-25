//! Restartable execution of an owner's approved Agent scope request.

use std::{sync::Arc, time::Duration};

use layer2_core::resource_access::CallerContext;
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::agent_io::{
    AgentCommandService, CommandAuthorizationError, PersonalCommandAdmissionAuthority,
};
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
mod updating;
mod waiting_quiet;

pub(crate) struct AgentAccessWorker {
    store: Arc<MongoSandboxStore>,
    auth: Arc<AuthState>,
    broker: Arc<dyn ServiceConnectionBroker>,
    github_broker: Option<Arc<crate::trigger::CmaServiceConnectionBroker>>,
    github_runtime: Option<Arc<dyn layer3_adapter::SandboxAdapter>>,
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
            github_broker: None,
            github_runtime: None,
            commands,
            holder,
        }
    }

    pub(crate) fn with_github_approval(
        mut self,
        broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
        runtime: Arc<dyn layer3_adapter::SandboxAdapter>,
    ) -> Self {
        self.github_broker = Some(broker);
        self.github_runtime = Some(runtime);
        self
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
                    _ = self.store.access_admitted.notified() => {
                        if self.tick().await.is_err() {
                            tracing::warn!("Agent access admitted work will retry");
                        }
                    }
                    _ = interval.tick() => {
                        if *shutdown.borrow() { break; }
                        // Drain bounded immediately ready durable phases; pending
                        // owner connections release their lease for other work.
                        if self.tick().await.is_err() {
                            tracing::warn!("Agent access workflow will retry a durable request");
                        }
                    }
                }
            }
        })
    }

    async fn tick(&self) -> Result<(), AgentAccessError> {
        let Some(mut record) = self.store.claim_access_request(&self.holder).await? else {
            return Ok(());
        };
        let lease = record.lease.clone().ok_or(AgentAccessError::Conflict)?;
        for _ in 0..12 {
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
                            self.store.renew_access_lease(&lease).await?;
                        }
                    }
                }
            };
            if result.is_ok() {
                let Some(current) = self.store.agent_access_request(&record.id).await? else {
                    return Err(AgentAccessError::NotFound);
                };
                if current.phase.terminal() {
                    self.store.access_admitted.notify_one();
                    return Ok(());
                }
                if current
                    .lease
                    .as_ref()
                    .is_none_or(|value| value.token != lease.token)
                {
                    return Err(AgentAccessError::Conflict);
                }
                if current.phase != record.phase || current.progress != record.progress {
                    record = current;
                    continue;
                }
            }
            match result {
                Ok(()) | Err(AgentAccessError::Conflict | AgentAccessError::Unavailable) => {
                    let delay = if matches!(record.phase, Phase::Updating | Phase::Notifying) {
                        Duration::from_millis(100)
                    } else {
                        Duration::from_secs(2)
                    };
                    match self.store.release_access_request(&lease, delay).await {
                        Ok(()) => {
                            self.store.access_admitted.notify_one();
                            if delay < Duration::from_secs(1) {
                                let wake = self.store.access_admitted.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(delay).await;
                                    wake.notify_one();
                                });
                            }
                            return Ok(());
                        }
                        Err(AgentAccessError::Conflict) => return Ok(()),
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => {
                    let code = match error {
                        AgentAccessError::Forbidden | AgentAccessError::OwnerAuthorityRequired => {
                            "owner_authority_revoked"
                        }
                        AgentAccessError::ServiceConnectionRequired => {
                            "service_connection_required"
                        }
                        AgentAccessError::NotFound => "agent_identity_changed",
                        _ => "access_update_cannot_continue",
                    };
                    self.store.fail_access_request(&lease, code).await?;
                    self.store.access_admitted.notify_one();
                    return Ok(());
                }
            }
        }
        self.store
            .release_access_request(&lease, Duration::ZERO)
            .await?;
        self.store.access_admitted.notify_one();
        Ok(())
    }

    async fn authority(
        &self,
        record: &AgentAccessRecord,
    ) -> Result<(CallerContext, SecretValue), AgentAccessError> {
        let Some(AgentAccessDecision::Approved { authorization, .. }) = &record.decision else {
            return Err(AgentAccessError::Forbidden);
        };
        self.auth
            .retained_owner_service_authority(&record.requester.owner, authorization)
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
        let services = record.approved_services()?;
        let unselected_services;
        let services = if let Some(connection) = record.approved_connection() {
            let broker = self
                .github_broker
                .as_ref()
                .ok_or(AgentAccessError::Unavailable)?;
            let (_, credential) = self.authority(record).await?;
            broker
                .verify_github_repo_token(&credential, &connection.connection_id)
                .await
                .map_err(crate::agent_access::github_proof_error)?;
            unselected_services = services
                .iter()
                .filter(|service| *service != &connection.service)
                .cloned()
                .collect::<Vec<_>>();
            if unselected_services.is_empty() {
                return Ok(true);
            }
            unselected_services.as_slice()
        } else {
            services
        };
        if matches!(&record.decision, Some(AgentAccessDecision::Approved { authorization, .. }) if authorization.authority_id.starts_with("owner-api:"))
        {
            let (context, credential) = self.authority(record).await?;
            self.auth
                .confirm_owner_service_scope(
                    &context,
                    credential.expose(),
                    &services.iter().map(ToString::to_string).collect::<Vec<_>>(),
                )
                .await?;
            return Ok(true);
        }
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
                    if sandbox.status == SandboxStatus::Active {
                        waiting_quiet::schedule(
                            &self.store,
                            record,
                            &sandbox,
                            self.github_runtime_ready(record, &sandbox),
                        )
                        .await?;
                    } else {
                        self.advance(record, Phase::WaitingQuiet).await?;
                    }
                }
            }
            Phase::WaitingQuiet => {
                waiting_quiet::schedule(&self.store, record, &sandbox, async {
                    Ok(self.connections_ready(record).await?
                        && self.github_runtime_ready(record, &sandbox).await?)
                })
                .await?;
            }
            Phase::Updating => self.updated(record, &agent, &sandbox).await?,
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

    async fn github_runtime_ready(
        &self,
        record: &AgentAccessRecord,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<bool, AgentAccessError> {
        if record.approved_connection().is_none() {
            return Ok(true);
        }
        let runtime = self
            .github_runtime
            .as_ref()
            .ok_or(AgentAccessError::Unavailable)?;
        let binding = sandbox.runtime.as_ref().ok_or(AgentAccessError::Conflict)?;
        let handle = layer3_adapter::RuntimeHandle::new(&binding.runtime_id);
        let status = tokio::time::timeout(Duration::from_secs(5), runtime.runtime_status(&handle))
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .map_err(|_| AgentAccessError::Unavailable)?;
        Ok(status.exact_github_credential_route
            && status.credential_ready
            && sandbox.runtime_process_matches(&status).unwrap_or(false))
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
