//! One bounded advisory field; strict domain records and work custody are unchanged.
use crate::sandbox::ownership_retry_hint::{
    AdvisoryDecision, CapturedOwnershipProbe, HintWriteResult, OwnershipRetryHints,
};
use crate::store::{agents::read_agent, sandbox_mongo::MongoSandboxStore, sandboxes::read_sandbox};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::{
    agent_ownership::OwnershipToken,
    sandbox::{Agent, AgentStatus, AgentWorkStep, Sandbox, SandboxStatus},
    AgentId,
};
use layer3_adapter::owner_protocol::{
    FirstOwnershipFailure, OwnershipFailureReason, OwnershipPhase, OWNER_PROTOCOL_VERSION,
    RECEIVER_LEASE_MS,
};
use mongodb::{
    bson::{doc, Bson, DateTime, Document},
    ClientSession,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(in crate::store) mod discovery;
pub(in crate::store) mod record;
#[cfg(test)]
mod tests;

pub(in crate::store) const FIELD: &str = "runtime_ownership_retry_v1";
const HOLD_MS: i64 = 60_000;
const MAX_BYTES: usize = 4096;
const BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

pub(super) fn policy_now(store: &MongoSandboxStore) -> Bson {
    #[cfg(test)]
    if let Some(now) = store.ownership_retry_hint_now {
        return Bson::DateTime(now);
    }
    let _ = store;
    Bson::String("$$NOW".into())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    owner: String,
    sandbox: String,
    workspace: String,
    worktree: String,
    conversation: String,
    generation: i64,
    ready_at: String,
    attempts: i64,
    command_generation: i64,
    runtime_id: String,
    boot_id: Option<String>,
    runtime_version: String,
    provision_generation: Option<i64>,
    bound_at: String,
    high_water: i64,
}

impl Key {
    fn capture(agent: &Agent, row: &Document, parent: &Sandbox) -> Option<Self> {
        if !eligible(agent, parent) {
            return None;
        }
        let runtime = parent.runtime.as_ref()?;
        let key = Self {
            owner: agent.owner.to_string(),
            sandbox: agent.sandbox_id.to_string(),
            workspace: agent.workspace_id.to_string(),
            worktree: agent.worktree_id.to_string(),
            conversation: agent.conversation.conversation_id.clone()?,
            generation: agent.conversation.runtime_generation?.try_into().ok()?,
            ready_at: agent.work.ready_at.clone(),
            attempts: agent.work.attempts.into(),
            command_generation: super::command_generation(row)?.try_into().ok()?,
            runtime_id: runtime.runtime_id.clone(),
            boot_id: runtime.boot_id.clone(),
            runtime_version: runtime.runtime_version.clone(),
            provision_generation: runtime
                .provision_generation
                .map(i64::try_from)
                .transpose()
                .ok()?,
            bound_at: runtime.bound_at.clone(),
            high_water: parent
                .lifecycle
                .runtime_generation_high_water
                .try_into()
                .ok()?,
        };
        key.valid().then_some(key)
    }

    fn valid(&self) -> bool {
        [
            &self.owner,
            &self.sandbox,
            &self.workspace,
            &self.worktree,
            &self.conversation,
            &self.ready_at,
            &self.runtime_id,
            &self.runtime_version,
            &self.bound_at,
        ]
        .iter()
        .all(|s| !s.is_empty() && s.len() <= 512)
            && self
                .boot_id
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 512)
            && self.generation > 0
            && self.command_generation >= 0
            && self.high_water >= 0
            && (0..=i64::from(u32::MAX)).contains(&self.attempts)
            && self.provision_generation.is_none_or(|n| n > 0)
    }
}

fn eligible(agent: &Agent, parent: &Sandbox) -> bool {
    matches!(
        agent.status,
        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
    ) && agent.work.step == AgentWorkStep::PublishConnection
        && agent.requested_stop_reason.is_none()
        && agent.deleted_at.is_none()
        && agent.sandbox_id == parent.id
        && agent.owner == parent.owner
        && parent.status == SandboxStatus::Active
        && parent.runtime.is_some()
        && parent.lifecycle.operation.is_none()
        && parent.lifecycle.checkpoint_capture.is_none()
        && parent.lifecycle.runtime_update.is_none()
        && parent.lifecycle.runtime_provision.is_none()
        && parent.lifecycle.suspended_runtime.is_none()
        && parent.lifecycle.home_recovery.is_none()
        && parent.lifecycle.unpublished_recovery.is_none()
        && !parent.retirement_archive_pending()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    CapabilityMissing,
    ChallengeProtocolMismatch,
    ChallengeLifetimeMismatch,
}

fn emit(event: &'static str, reason: Option<Reason>) {
    // JSON tracing includes the current span even for parent:none events.
    // Enter a detached, field-free root so request payloads cannot be inherited.
    let root =
        tracing::error_span!(target: "ownership_retry_hint", parent: None, "ownership_retry_hint");
    if root.is_disabled() {
        return;
    }
    let _entered = root.enter();
    let reason = reason.map(|reason| match reason {
        Reason::CapabilityMissing => "capability_missing",
        Reason::ChallengeProtocolMismatch => "challenge_protocol_mismatch",
        Reason::ChallengeLifetimeMismatch => "challenge_lifetime_mismatch",
    });
    if event == "unavailable" {
        tracing::warn!(target: "ownership_retry_hint", parent: None,
            event, reason, phase = "challenge", duration_ms = HOLD_MS,
            "Ownership reconnect hint unavailable");
    } else {
        tracing::debug!(target: "ownership_retry_hint", parent: None,
            event, reason, phase = "challenge", duration_ms = HOLD_MS,
            "Ownership reconnect hint decision");
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    version: i64,
    observation: String,
    reason: Reason,
    key: Key,
    owner_protocol: i64,
    receiver_lease_ms: i64,
    retry_classifier_version: i64,
    observed_at: DateTime,
    until: DateTime,
    owner_epoch: i64,
    claimed_revision: i64,
}

impl Hint {
    fn read(row: &Document) -> Option<Self> {
        let value = row.get_document(FIELD).ok()?;
        if value.len() != 11 || value.get_document("key").ok()?.len() != 15 {
            return None;
        }
        if mongodb::bson::serialize_to_vec(value).ok()?.len() > MAX_BYTES {
            return None;
        }
        let hint: Self = mongodb::bson::deserialize_from_document(value.clone()).ok()?;
        (hint.version == 1
            && hint.retry_classifier_version == 1
            && hint.owner_protocol == i64::from(OWNER_PROTOCOL_VERSION)
            && hint.receiver_lease_ms == RECEIVER_LEASE_MS as i64
            && hint.owner_epoch > 0
            && hint.claimed_revision > 0
            && uuid::Uuid::parse_str(&hint.observation)
                .is_ok_and(|id| id.to_string() == hint.observation)
            && hint.key.valid()
            && hint
                .until
                .timestamp_millis()
                .checked_sub(hint.observed_at.timestamp_millis())
                == Some(HOLD_MS))
        .then_some(hint)
    }
    fn active(&self, now: DateTime) -> bool {
        self.observed_at <= now && now < self.until
    }
}

pub(in crate::store) fn holds(
    row: &Document,
    agent: &Agent,
    parent: &Sandbox,
    now: DateTime,
) -> bool {
    let Some(hint) = Hint::read(row) else {
        if row.contains_key(FIELD) {
            emit("malformed", None);
        }
        return false;
    };
    if !hint.active(now) {
        emit(
            if now >= hint.until {
                "expired"
            } else {
                "malformed"
            },
            Some(hint.reason),
        );
        return false;
    }
    let active = Key::capture(agent, row, parent).as_ref() == Some(&hint.key);
    emit(
        if active { "held" } else { "mismatched" },
        Some(hint.reason),
    );
    active
}

impl MongoSandboxStore {
    pub(crate) fn with_ownership_retry_hints(mut self, enabled: bool) -> Self {
        self.ownership_retry_hints_enabled = enabled;
        self
    }
    pub(crate) fn ownership_retry_hints(self: &Arc<Self>) -> Option<Arc<dyn OwnershipRetryHints>> {
        self.ownership_retry_hints_active()
            .then(|| self.clone() as Arc<dyn OwnershipRetryHints>)
    }
    pub(in crate::store) fn ownership_retry_hints_active(&self) -> bool {
        self.ownership_retry_hints_enabled && self.agent_ownership.is_some()
    }
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

/// Same server-time seam as discovery, including inside the claim transaction.
pub(in crate::store) async fn server_row(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<Option<(Document, DateTime)>, SandboxStoreError> {
    let mut cursor = store
        .agents
        .aggregate(vec![
            doc! {"$match": {"_id": id.as_str()}},
            doc! {"$set": {"_ownership_retry_now": policy_now(store)}},
        ])
        .max_time(BUDGET)
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    if !cursor.advance(session).await.map_err(unavailable)? {
        return Ok(None);
    }
    let mut row = cursor.deserialize_current().map_err(unavailable)?;
    let now = row
        .get_datetime("_ownership_retry_now")
        .copied()
        .map_err(unavailable)?;
    row.remove("_ownership_retry_now");
    Ok(Some((row, now)))
}

#[async_trait::async_trait]
impl OwnershipRetryHints for MongoSandboxStore {
    async fn preflight(&self, id: &AgentId) -> AdvisoryDecision {
        if !self.ownership_retry_hints_active() {
            return AdvisoryDecision::Proceed;
        }
        let result = tokio::time::timeout(BUDGET, async {
            let mut session = self
                .client
                .start_session()
                .snapshot(true)
                .await
                .map_err(unavailable)?;
            let Some((row, now)) = server_row(self, id, &mut session).await? else {
                return Ok(false);
            };
            let agent = read_agent(&row)?;
            let Some(hint) = Hint::read(&row) else {
                if row.contains_key(FIELD) {
                    emit("malformed", None);
                }
                return Ok(false);
            };
            if !hint.active(now) {
                emit(
                    if now >= hint.until {
                        "expired"
                    } else {
                        "malformed"
                    },
                    Some(hint.reason),
                );
                return Ok(false);
            }
            let Some(parent) = self
                .sandboxes
                .find_one(doc! {"_id": agent.sandbox_id.as_str()})
                .session(&mut session)
                .await
                .map_err(unavailable)?
            else {
                return Ok(false);
            };
            Ok::<_, SandboxStoreError>(holds(&row, &agent, &read_sandbox(&parent)?, now))
        })
        .await;
        match result {
            Ok(Ok(true)) => AdvisoryDecision::Hold,
            Ok(Ok(false)) => AdvisoryDecision::Proceed,
            _ => {
                emit("unavailable", None);
                AdvisoryDecision::Unavailable
            }
        }
    }

    async fn capture(
        &self,
        claimed: &Agent,
        owner: &OwnershipToken,
    ) -> Option<Arc<dyn CapturedOwnershipProbe>> {
        if !self.ownership_retry_hints_active() || owner.agent_id != claimed.id {
            return None;
        }
        match tokio::time::timeout(BUDGET, record::capture(self, claimed, owner)).await {
            Ok(Ok(observation)) => {
                observation.map(|o| Arc::new(o) as Arc<dyn CapturedOwnershipProbe>)
            }
            _ => {
                emit("unavailable", None);
                None
            }
        }
    }
}
