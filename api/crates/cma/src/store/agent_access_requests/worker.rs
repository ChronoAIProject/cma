use super::*;
use std::time::Duration;

const LEASE_MILLIS: i64 = 90_000;

impl MongoSandboxStore {
    #[cfg(test)]
    pub(crate) async fn read_access_request(
        &self,
        id: &str,
    ) -> Result<Option<AgentAccessRecord>, AgentAccessError> {
        self.agent_access_request(id).await
    }

    pub(crate) async fn verify_access_request_lineage(
        &self,
        record: &AgentAccessRecord,
    ) -> Result<(layer2_core::sandbox::Agent, layer2_core::sandbox::Sandbox), AgentAccessError>
    {
        transaction!(self, session, {
            let snapshot =
                validate_lineage_transaction(self, &record.requester, session, false).await?;
            fence(self, &snapshot, session).await?;
            Ok((
                snapshot
                    .resource
                    .lineage
                    .agent
                    .ok_or(AgentAccessError::NotFound)?,
                snapshot.resource.lineage.sandbox,
            ))
        })
    }

    pub(crate) async fn claim_access_request(
        &self,
        holder: &str,
    ) -> Result<Option<AgentAccessRecord>, AgentAccessError> {
        if holder.trim().is_empty() || holder.len() > 128 {
            return Err(AgentAccessError::Invalid);
        }
        if self
            .empty_discovery
            .empty(
                super::super::empty_discovery::Kind::Access,
                &self.agent_access_requests,
                doc! {"record.phase":{"$in":ACTIVE}},
            )
            .await
        {
            return Ok(None);
        }
        let now = now_millis();
        let expiry =
            layer2_core::rfc3339_at(std::time::SystemTime::now() - Duration::from_secs(24 * 3600));
        transaction!(self, session, {
            let Some(value) = self.agent_access_requests.find_one(doc! {
                "record.phase":{"$in":ACTIVE}, "record.retry_at":{"$lte":now},
                "$and":[
                    {"$or":[{"record.phase":{"$ne":"pending"}},{"record.created_at":{"$lte":&expiry}}]},
                    {"$or":[{"record.lease":null},{"record.lease.expires_at":{"$lte":now}}]},
                ],
            }).sort(doc! {"record.retry_at":1,"record.created_at":1,"_id":1}).session(&mut *session).await? else {
                self.empty_discovery.observed(super::super::empty_discovery::Kind::Access, true);
                return Ok(None);
            };
            self.empty_discovery
                .observed(super::super::empty_discovery::Kind::Access, false);
            let mut record = read(&value)?;
            let revision = record.revision;
            record.revision += 1;
            record.updated_at = layer2_core::now_rfc3339();
            record.lease = Some(AgentAccessLease {
                id: record.id.clone(),
                token: uuid::Uuid::new_v4().to_string(),
                holder: holder.into(),
                expires_at: now + LEASE_MILLIS,
            });
            replace(self, revision, &record, session).await?;
            Ok(Some(record))
        })
    }

    pub(crate) async fn renew_access_lease(
        &self,
        lease: &AgentAccessLease,
    ) -> Result<AgentAccessLease, AgentAccessError> {
        transaction!(self, session, {
            let mut record = leased(self, lease, session).await?;
            let revision = record.revision;
            record.revision += 1;
            let current = record.lease.as_mut().ok_or(AgentAccessError::Conflict)?;
            current.expires_at = now_millis() + LEASE_MILLIS;
            let renewed = current.clone();
            replace_leased(self, lease, revision, &record, session).await?;
            Ok(renewed)
        })
    }

    pub(crate) async fn advance_access_request(
        &self,
        lease: &AgentAccessLease,
        expected: AgentAccessPhase,
        next: AgentAccessPhase,
        progress: AgentAccessProgress,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        if !legal_transition(expected, next) {
            return Err(AgentAccessError::Invalid);
        }
        transaction!(self, session, {
            let mut record = leased(self, lease, session).await?;
            if record.phase != expected || !progress.extends(&record.progress) {
                return Err(AgentAccessError::Conflict.into());
            }
            let snapshot =
                validate_lineage_transaction(self, &record.requester, session, false).await?;
            validate_progress(&record, next, &progress, &snapshot)?;
            if next.applying() && self.agent_access_requests.count_documents(doc! {
                "_id":{"$ne":&record.id},"record.requester.sandbox_id":record.requester.sandbox_id.as_str(),
                "record.phase":{"$in":APPLYING},
            }).session(&mut *session).await? > 0 { return Err(AgentAccessError::Conflict.into()); }
            fence(self, &snapshot, session).await?;
            let revision = record.revision;
            record.revision += 1;
            record.phase = next;
            record.progress = progress.clone();
            record.updated_at = layer2_core::now_rfc3339();
            if next.terminal() {
                record.lease = None;
            }
            replace_leased(self, lease, revision, &record, session).await?;
            Ok(record)
        })
    }

    pub(crate) async fn release_access_request(
        &self,
        lease: &AgentAccessLease,
        retry_after: Duration,
    ) -> Result<(), AgentAccessError> {
        let delay =
            i64::try_from(retry_after.as_millis()).map_err(|_| AgentAccessError::Invalid)?;
        if delay > 24 * 3600 * 1000 {
            return Err(AgentAccessError::Invalid);
        }
        transaction!(self, session, {
            let mut record = leased(self, lease, session).await?;
            let revision = record.revision;
            record.revision += 1;
            record.lease = None;
            record.retry_at = now_millis() + delay;
            record.updated_at = layer2_core::now_rfc3339();
            replace_leased(self, lease, revision, &record, session).await?;
            Ok(())
        })
    }

    pub(crate) async fn fail_access_request(
        &self,
        lease: &AgentAccessLease,
        code: &str,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        if code.is_empty()
            || code.len() > 96
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            return Err(AgentAccessError::Invalid);
        }
        transaction!(self, session, {
            let mut record = leased(self, lease, session).await?;
            let revision = record.revision;
            record.revision += 1;
            record.phase = AgentAccessPhase::Failed;
            record.lease = None;
            record.error = Some(code.into());
            record.updated_at = layer2_core::now_rfc3339();
            replace_leased(self, lease, revision, &record, session).await?;
            Ok(record)
        })
    }
}
pub(super) async fn leased(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    session: &mut ClientSession,
) -> TxResult<AgentAccessRecord> {
    let value = store
        .agent_access_requests
        .find_one(lease_filter(lease, now_millis()))
        .session(session)
        .await?
        .ok_or(AgentAccessError::Conflict)?;
    let record = read(&value)?;
    if record.phase.terminal() {
        return Err(AgentAccessError::Conflict.into());
    }
    Ok(record)
}
fn legal_transition(from: AgentAccessPhase, to: AgentAccessPhase) -> bool {
    use AgentAccessPhase::*;
    from == to && from == Notifying
        || matches!(
            (from, to),
            (Connecting, WaitingQuiet)
                | (Pausing, Granting)
                | (Reviving, Notifying)
                | (Updating, Notifying)
                | (Notifying, Succeeded)
        )
}

fn validate_progress(
    record: &AgentAccessRecord,
    next: AgentAccessPhase,
    progress: &AgentAccessProgress,
    snapshot: &layer2_core::sandbox::store::AgentProvenanceSnapshot,
) -> Result<(), AgentAccessError> {
    use layer2_core::sandbox::{SandboxStatus, SandboxWorkStep};
    let sandbox = &snapshot.resource.lineage.sandbox;
    let latest_operation = format!(
        "sandbox-lifecycle-v1:{}:{}",
        sandbox.id.as_str(),
        sandbox.lifecycle.next_authority.saturating_sub(1)
    );
    if next == AgentAccessPhase::WaitingQuiet && *progress != AgentAccessProgress::default() {
        return Err(AgentAccessError::Conflict);
    }
    if next == AgentAccessPhase::Granting
        && (progress != &record.progress
            || sandbox.status != SandboxStatus::Paused
            || sandbox.runtime.is_some()
            || sandbox.work.step != SandboxWorkStep::None
            || sandbox.work.lease.is_some()
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.credential.pending.is_some()
            || progress.pause_operation_id.as_deref() != Some(&latest_operation))
    {
        return Err(AgentAccessError::Conflict);
    }
    if matches!(
        record.phase,
        AgentAccessPhase::Reviving | AgentAccessPhase::Updating
    ) && next == AgentAccessPhase::Notifying
    {
        let floor = progress
            .grant_generation_floor
            .ok_or(AgentAccessError::Conflict)?;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(AgentAccessError::NotFound)?;
        let services = sandbox.credential.effective_reaches();
        if sandbox.status != SandboxStatus::Active
            || sandbox.lifecycle.operation.is_some()
            || !matches!(
                sandbox.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || (record.phase == AgentAccessPhase::Reviving
                && progress.revive_operation_id.as_deref() != Some(&latest_operation))
            || (record.phase == AgentAccessPhase::Updating
                && progress.hot_update.as_ref().is_none_or(|hot| {
                    sandbox.runtime.as_ref() != Some(&hot.runtime)
                        || sandbox.lifecycle.next_authority != hot.lifecycle_next_authority
                }))
            || (record.phase == AgentAccessPhase::Updating
                && (sandbox.work.lease.is_some()
                    || sandbox.lifecycle.checkpoint_capture.is_some()
                    || !sandbox.credential.revoke_debt.is_empty()
                    || sandbox
                        .credential
                        .service_scope
                        .as_ref()
                        .is_none_or(|scope| {
                            Some(scope.revision)
                                != record.expected_scope_revision.unwrap_or(0).checked_add(1)
                                || scope.authorized_by != record.requester.owner
                                || scope.credential_generation_floor != floor
                                || !record
                                    .approved_services()
                                    .is_ok_and(|approved| scope.services == approved)
                                || record.approved_connection().is_some_and(|selected| {
                                    scope.connection.as_ref() != Some(selected)
                                })
                        })))
            || !sandbox.credential.delivered
            || sandbox.credential.pending.is_some()
            || sandbox.credential.current.as_ref() != progress.delivered_credential.as_ref()
            || progress
                .delivered_credential
                .as_ref()
                .is_none_or(|credential| credential.generation <= floor)
            || sandbox.runtime.as_ref() != progress.delivered_runtime.as_ref()
            || sandbox.runtime.as_ref().is_none_or(|runtime| {
                runtime.provision_generation != agent.conversation.runtime_generation
            })
            || !record
                .approved_services()?
                .iter()
                .all(|service| services.contains(service))
        {
            return Err(AgentAccessError::Conflict);
        }
    }
    Ok(())
}
