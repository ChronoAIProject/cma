//! Mongo work discovery and fenced lease claims for agents.

use layer2_core::ids::AgentId;
use layer2_core::sandbox::store::{
    apply_agent_connection_recovery, AgentConnectionRecoveryApplication,
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome,
    AgentConnectionRecoveryRequest, AgentStore, AgentWorkClaimDetail as ClaimDetail,
    AgentWorkClaimError, AgentWorkDiscovery, AgentWorkMutation, AgentWorkerStore, Page,
    PageRequest, SandboxStoreError, StartupWorkFilter, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::doc;
use std::time::Duration;

use super::child_writer_claims::{persist_observed, ChildWriterClaim};
use super::sandbox_mongo::{MongoSandboxStore, TRANSACTION_ATTEMPTS};

mod creation_admission;

#[async_trait::async_trait]
impl AgentWorkerStore for MongoSandboxStore {
    async fn agent_restart_teardown(
        &self,
        claimed: &Agent,
    ) -> Result<layer2_core::sandbox::store::AgentRestartTeardown, SandboxStoreError> {
        super::agent_control_transaction::restart_teardown::load(self, claimed).await
    }

    async fn publish_agent_restart_teardown(
        &self,
        publication: layer2_core::sandbox::store::AgentRestartTeardownPublication,
    ) -> Result<(), SandboxStoreError> {
        super::agent_control_transaction::restart_teardown::publish(self, &publication).await
    }

    async fn agent_restart_work(
        &self,
        claimed: &Agent,
    ) -> Result<layer2_core::sandbox::store::AgentRestartWork, SandboxStoreError> {
        super::agent_control_transaction::load_restart_work(self, claimed).await
    }

    async fn restart_connection_open(
        &self,
        agent: &Agent,
    ) -> Result<Option<layer3_adapter::AgentOpenRequest>, SandboxStoreError> {
        super::agent_control_transaction::load_restart_connection(self, agent).await
    }

    async fn prepare_agent_restart_dispatch(
        &self,
        request: layer2_core::sandbox::store::AgentRestartDispatch,
    ) -> Result<Agent, SandboxStoreError> {
        super::agent_control_transaction::prepare_restart_dispatch(self, &request).await
    }

    async fn has_owner_stop_receipt(
        &self,
        agent: &layer2_core::sandbox::Agent,
    ) -> Result<bool, SandboxStoreError> {
        super::agent_control_receipt::observe_owner_stop_receipt(self, agent).await
    }

    async fn agent_create_admission(
        &self,
        agent: &layer2_core::sandbox::Agent,
    ) -> Result<Option<layer2_core::sandbox::store::AgentCreateAdmission>, SandboxStoreError> {
        creation_admission::read(self, agent).await
    }

    async fn agent_command_admission_generation(
        &self,
        id: &AgentId,
    ) -> Result<Option<u64>, SandboxStoreError> {
        let row = self
            .agents
            .find_one(doc! {"_id": id.as_str()})
            .projection(doc! {"agent_command_admission_fence": 1,
            "document_schema_version": 1, "agent_log_format": 1,
            "agent_log_incarnation": 1, "agent_log_head_revision": 1})
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let Some(row) = row else {
            return Ok(None);
        };
        // This is only a pacing hint. Validate the envelope discriminator;
        // admission and publication still establish their own current authority.
        super::agent_log_segments::read_format(&row)?;
        Ok(super::agent_failure_pacing::command_generation(&row))
    }

    async fn commit_agent_work_retry(
        &self,
        mutation: AgentWorkMutation,
        observed: layer2_core::sandbox::store::AgentRetryObservation,
    ) -> Result<(), SandboxStoreError> {
        super::agent_worker_transaction::commit_agent_work(self, mutation.clone()).await?;
        super::agent_failure_pacing::record(self, &mutation, &observed).await
    }

    async fn try_publish_agent_work(
        &self,
        mutation: AgentWorkMutation,
        parents: layer2_core::sandbox::store::AgentWorkPublicationParents,
    ) -> Result<layer2_core::sandbox::store::AgentWorkPublication, SandboxStoreError> {
        super::agent_worker_transaction::try_publish_agent_work(self, mutation, parents).await
    }

    async fn commit_agent_work_repeated_failure(
        &self,
        retry: layer2_core::sandbox::store::AgentRepeatedFailure,
        observed: Option<layer2_core::sandbox::store::AgentRetryObservation>,
    ) -> Result<(), SandboxStoreError> {
        super::agent_worker_transaction::commit_repeated_failure(self, &retry).await?;
        if let Some(observed) = observed {
            super::agent_failure_pacing::record_with_marker(
                self,
                &retry.mutation,
                &observed,
                Some(&retry.marker.kind),
            )
            .await?;
        }
        Ok(())
    }

    async fn prepared_agent_replay_matches(
        &self,
        agent: &layer2_core::sandbox::Agent,
        sandbox: &layer2_core::sandbox::Sandbox,
        workspace: &layer2_core::sandbox::Workspace,
        worktree: &layer2_core::sandbox::Worktree,
    ) -> Result<bool, SandboxStoreError> {
        if agent.work.warm_conversation_id.is_none() {
            return Ok(false);
        }
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        Ok(super::native_prepared::replay_proof_revision(
            self,
            agent,
            sandbox,
            workspace,
            worktree,
            &mut session,
        )
        .await?
        .is_some())
    }

    async fn commit_prepared_agent_open_ready(
        &self,
        mutation: AgentWorkMutation,
        observed: layer2_core::sandbox::store::PreparedAgentOpenContext,
    ) -> Result<bool, SandboxStoreError> {
        super::agent_worker_transaction::commit_prepared_agent_open_ready(self, mutation, observed)
            .await
    }

    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! {
            "document_schema_version": { "$in": [1_i64, 2_i64] },
            "status": { "$in": super::agents::LIVE_STATUSES },
            "record.conversation.conversation_id": { "$type": "string", "$ne": "" },
            "record.conversation.runtime_generation": { "$gt": 0_i64 },
        };
        if let Some(cursor) = page.cursor.as_ref() {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut read = super::mongo_read_telemetry::Read::find(self.agents.name());
        let mut pass = self
            .agent_connection_observations
            .as_ref()
            .map(|cache| cache.pass());
        let mut cursor = self
            .agents
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while cursor
            .advance()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            read.document(cursor.current().as_bytes().len());
            let validate = || {
                let document = cursor
                    .deserialize_current()
                    .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
                let agent = super::agents::read_agent(&document)?;
                AgentConnectionRecoveryCandidate::from_agent(&agent).ok_or_else(|| {
                    SandboxStoreError::Corrupt(
                        "agent connection query returned an invalid candidate".into(),
                    )
                })
            };
            let candidate = match &self.agent_connection_observations {
                Some(cache) => cache.resolve(cursor.current(), validate)?,
                None => validate()?,
            };
            items.push(candidate);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded connection page has a last item")
                .agent_id
                .to_string()
        });
        read.complete();
        if let Some(pass) = pass.as_mut() {
            pass.complete = true;
        }
        Ok(Page { items, next })
    }

    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError> {
        let current = self
            .get_agent(&request.agent_id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        match apply_agent_connection_recovery(&current, &request)? {
            AgentConnectionRecoveryApplication::Complete(outcome) => Ok(outcome),
            AgentConnectionRecoveryApplication::Persist(agent) => {
                match super::agents::save::save_connection_recovery(self, &current, *agent).await {
                    Ok(()) => Ok(AgentConnectionRecoveryOutcome::Scheduled),
                    Err(SandboxStoreError::Conflict) => {
                        let current = self
                            .get_agent(&request.agent_id)
                            .await?
                            .ok_or(SandboxStoreError::NotFound)?;
                        match apply_agent_connection_recovery(&current, &request)? {
                            AgentConnectionRecoveryApplication::Complete(outcome) => Ok(outcome),
                            AgentConnectionRecoveryApplication::Persist(_) => {
                                Ok(AgentConnectionRecoveryOutcome::Superseded)
                            }
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    async fn due_agent_work_filtered(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let unrestricted = matches!(filter.startup, StartupWorkFilter::All)
            && filter.acquisition_cooldowns.is_none();
        if unrestricted
            && self
                .empty_discovery
                .empty(
                    super::empty_discovery::Kind::Agents,
                    &self.agents,
                    doc! {"work_pending":true},
                )
                .await
        {
            return Ok(Vec::new());
        }
        if self.agent_ownership.is_some() {
            let ids = if self.ownership_retry_hints_active() {
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    self.due_owned_agent_work(now, limit, filter),
                )
                .await
                .map_err(|_| {
                    SandboxStoreError::Unavailable(
                        "ownership hint discovery exceeded its deadline".into(),
                    )
                })??
            } else {
                self.due_owned_agent_work(now, limit, filter).await?
            };
            if unrestricted {
                self.empty_discovery
                    .observed(super::empty_discovery::Kind::Agents, ids.is_empty());
            }
            return Ok(ids);
        }
        let mut read = super::mongo_read_telemetry::Read::find(self.agents.name());
        let mut cursor = self
            .agents
            .aggregate(super::agent_failure_pacing::discovery::filtered_pipeline(
                self, now, limit, filter,
            ))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut ids = Vec::new();
        while let Some(document) = read
            .next(&mut cursor)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            ids.push(AgentId(
                document
                    .get_str("_id")
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .to_string(),
            ));
        }
        read.complete();
        if unrestricted {
            self.empty_discovery
                .observed(super::empty_discovery::Kind::Agents, ids.is_empty());
        }
        Ok(ids)
    }

    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<layer2_core::sandbox::Agent>, SandboxStoreError> {
        self.claim_agent_work_observed(id, holder, now, lease_expires_at)
            .await
            .map_err(|error| error.store_error)
    }

    fn agent_inspection_parking_enabled(&self) -> bool {
        self.agent_inspection_parking
    }

    async fn agent_inspection_parked(&self, id: &AgentId) -> Result<bool, SandboxStoreError> {
        super::agent_inspection_park::parked(self, id).await
    }

    async fn park_agent_inspection(
        &self,
        owner: &layer2_core::agent_ownership::OwnershipToken,
        claimed: &Agent,
        binding: &layer2_core::sandbox::RuntimeBinding,
        failure: layer2_core::agent_ownership::OwnershipFailure,
    ) -> Result<bool, SandboxStoreError> {
        super::agent_inspection_park::park(self, owner, claimed, binding, failure).await
    }

    async fn confirm_agent_inspection(
        &self,
        claimed: &Agent,
        binding: &layer2_core::sandbox::RuntimeBinding,
    ) -> Result<(), SandboxStoreError> {
        super::agent_inspection_park::confirm(self, claimed, binding).await
    }

    async fn claim_agent_work_observed(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<layer2_core::sandbox::Agent>, AgentWorkClaimError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let Some(row) = self
                .agents
                .find_one(doc! {"_id": id.as_str()})
                .await
                .map_err(|error| {
                    AgentWorkClaimError::with_detail(
                        SandboxStoreError::Unavailable(error.to_string()),
                        ClaimDetail::Read,
                    )
                })?
            else {
                return Ok(None);
            };
            if super::agent_inspection_park::has_marker(&row)
                && self.agent_inspection_parked(id).await?
            {
                return Ok(None);
            }
            let mut agent = super::agents::read_agent(&row)?;
            if super::agent_worker_transaction::pending_adoption::witness::blocks_claim(
                self, &row, &agent,
            )
            .map_err(|error| {
                AgentWorkClaimError::with_detail(error, ClaimDetail::PublicationCustody)
            })? {
                let retired =
                    super::agent_worker_transaction::pending_adoption::retirement::retire_expired(
                        self, &row, &agent, &now,
                    )
                    .await
                    .map_err(|error| {
                        AgentWorkClaimError::with_detail(error, ClaimDetail::PublicationCustody)
                    })?;
                if retired && self.segment_adoption_enabled {
                    continue;
                }
                return Ok(None);
            }
            if self.ownership_retry_hints_active() {
                use crate::sandbox::ownership_retry_hint::{AdvisoryDecision, OwnershipRetryHints};
                if self.preflight(id).await == AdvisoryDecision::Hold {
                    return Ok(None);
                }
            }
            self.recover_agent_work_owner(&mut agent)
                .await
                .map_err(|error| AgentWorkClaimError::with_detail(error, ClaimDetail::Read))?;
            if agent
                .work
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now)
            {
                return Ok(None);
            }
            match super::agent_failure_pacing::decision(self, &row, &agent, &now)
                .await
                .map_err(|error| AgentWorkClaimError::with_detail(error, ClaimDetail::Read))?
            {
                super::agent_failure_pacing::Decision::Hold => return Ok(None),
                super::agent_failure_pacing::Decision::Wake => {
                    agent.work.ready_at = now.clone();
                }
                super::agent_failure_pacing::Decision::Ordinary => {}
            }
            if !agent.work.is_due(&now) {
                return Ok(None);
            }
            let expected = agent.revision;
            agent
                .work
                .claim(holder.to_string(), &now, lease_expires_at.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            match persist_observed(
                self,
                expected,
                ChildWriterClaim::Agent(Box::new(agent.clone())),
            )
            .await
            {
                Ok(true) => return Ok(Some(agent)),
                Ok(false) => return Ok(None),
                Err(error) if matches!(error.store_error, SandboxStoreError::Conflict) => {
                    tokio::task::yield_now().await
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict.into())
    }

    async fn commit_agent_skill_result(
        &self,
        mutation: AgentWorkMutation,
        guard: layer2_core::sandbox::store::AgentSkillResultGuard,
    ) -> Result<(), SandboxStoreError> {
        super::agent_worker_transaction::commit_agent_skill_result(self, mutation, &guard).await
    }

    async fn commit_agent_work(
        &self,
        mutation: AgentWorkMutation,
    ) -> Result<(), SandboxStoreError> {
        super::agent_worker_transaction::commit_agent_work(self, mutation).await
    }

    async fn commit_agent_connection_work(
        &self,
        mutation: AgentWorkMutation,
        parents: layer2_core::sandbox::store::AgentWorkPublicationParents,
    ) -> Result<(), SandboxStoreError> {
        super::agent_worker_transaction::commit_agent_connection_work(self, mutation, parents).await
    }
}
