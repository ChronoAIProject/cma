use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_access::{AgentAccessError, AgentAccessRequest, AgentAccessStatus};
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::{
    RuntimeBinding, SandboxCredentialAuthority, ServiceConnectionSelection,
};
use layer2_core::{SandboxId, ServiceId};
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

use super::valid_owner_authorization;
use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentAccessPhase {
    Pending,
    Connecting,
    WaitingQuiet,
    Updating,
    Pausing,
    Granting,
    Reviving,
    Notifying,
    Succeeded,
    Denied,
    Failed,
}
impl AgentAccessPhase {
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Denied | Self::Failed)
    }
    pub(crate) fn applying(self) -> bool {
        matches!(
            self,
            Self::WaitingQuiet
                | Self::Updating
                | Self::Pausing
                | Self::Granting
                | Self::Reviving
                | Self::Notifying
        )
    }
    pub(crate) fn status(self) -> AgentAccessStatus {
        match self {
            Self::Pending => AgentAccessStatus::Pending,
            Self::Connecting => AgentAccessStatus::Connecting,
            Self::Succeeded => AgentAccessStatus::Succeeded,
            Self::Denied => AgentAccessStatus::Denied,
            Self::Failed => AgentAccessStatus::Failed,
            _ => AgentAccessStatus::Applying,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AgentAccessDecision {
    Approved {
        command_key_hash: String,
        services: Vec<ServiceId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connection: Option<ServiceConnectionSelection>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connection_binding: Option<ConnectionApprovalBinding>,
        authorization: AgentAuthorizationRevision,
    },
    Denied {
        command_key_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectionApprovalBinding {
    pub sandbox_id: SandboxId,
    pub scope_revision: u64,
    pub credential_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessLease {
    pub id: String,
    pub token: String,
    pub holder: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessHotUpdate {
    pub runtime: RuntimeBinding,
    pub lifecycle_next_authority: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessNotificationIntent {
    pub attempt: u32,
    pub agent_revision: u64,
    pub target_turn: Option<u64>,
    pub previous_commands: Vec<AgentCommandId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessProgress {
    /// Fresh owner consent for this exact scope; the earlier audit stays intact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_intent: Option<AgentAccessNotificationIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_update: Option<AgentAccessHotUpdate>,
    pub pause_operation_id: Option<String>,
    pub grant_generation_floor: Option<u64>,
    pub revive_operation_id: Option<String>,
    pub delivered_credential: Option<SandboxCredentialAuthority>,
    pub delivered_runtime: Option<RuntimeBinding>,
    pub notification_command_id: Option<AgentCommandId>,
}
impl AgentAccessProgress {
    pub(super) fn extends(&self, previous: &Self) -> bool {
        self.renewed_from == previous.renewed_from
            && (previous.notification_intent.is_none()
                || self.notification_intent == previous.notification_intent)
            && (previous.hot_update.is_none() || self.hot_update == previous.hot_update)
            && (previous.pause_operation_id.is_none()
                || self.pause_operation_id == previous.pause_operation_id)
            && (previous.grant_generation_floor.is_none()
                || self.grant_generation_floor == previous.grant_generation_floor)
            && (previous.revive_operation_id.is_none()
                || self.revive_operation_id == previous.revive_operation_id)
            && (previous.delivered_credential.is_none()
                || self.delivered_credential == previous.delivered_credential)
            && (previous.delivered_runtime.is_none()
                || self.delivered_runtime == previous.delivered_runtime)
            && (previous.notification_command_id.is_none()
                || self.notification_command_id == previous.notification_command_id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum AgentAccessScopeMode {
    #[default]
    Additive,
    Replace {
        expected_services: Vec<ServiceId>,
    },
}
impl AgentAccessScopeMode {
    fn is_additive(&self) -> bool {
        matches!(self, Self::Additive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessRecord {
    #[serde(default, skip_serializing_if = "AgentAccessScopeMode::is_additive")]
    pub scope_mode: AgentAccessScopeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_scope_revision: Option<u64>,
    pub id: String,
    pub requester: AgentAccessRequester,
    pub proposal: AgentAccessProposal,
    pub body_sha256: String,
    pub phase: AgentAccessPhase,
    pub decision: Option<AgentAccessDecision>,
    pub lease: Option<AgentAccessLease>,
    pub progress: AgentAccessProgress,
    pub approved_at: Option<String>,
    pub retry_at: i64,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub error: Option<String>,
}
impl AgentAccessRecord {
    pub(crate) fn view(&self) -> AgentAccessRequest {
        AgentAccessRequest {
            id: self.id.clone(),
            agent_id: self.requester.agent_id.clone(),
            status: self.phase.status(),
            services: self.proposal.services.clone(),
            reason: self.proposal.reason.clone(),
            retry_of: self.proposal.retry_of.clone(),
            error: self.error.clone(),
            github_approval: None,
        }
    }

    pub(crate) fn approved_services(&self) -> Result<&[ServiceId], AgentAccessError> {
        match &self.decision {
            Some(AgentAccessDecision::Approved { services, .. }) => Ok(services),
            _ => Err(AgentAccessError::Conflict),
        }
    }

    pub(crate) fn approved_connection(&self) -> Option<&ServiceConnectionSelection> {
        match &self.decision {
            Some(AgentAccessDecision::Approved { connection, .. }) => connection.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn approved_connection_binding(&self) -> Option<&ConnectionApprovalBinding> {
        match &self.decision {
            Some(AgentAccessDecision::Approved {
                connection_binding, ..
            }) => connection_binding.as_ref(),
            _ => None,
        }
    }

    fn validate(&self) -> Result<(), AgentAccessError> {
        self.proposal.validate()?;
        let identity = &self.requester;
        let approved = matches!(self.decision, Some(AgentAccessDecision::Approved { .. }));
        if self.revision == 0
            || self.id != request_id(identity, &self.proposal.request_key)?
            || self.proposal.retry_of.as_deref() == Some(&self.id)
            || self.body_sha256 != digest(&self.proposal)?
            || identity.agent_id != identity.creation.agent_id
            || identity.sandbox_id != identity.creation.sandbox_id
            || identity.conversation.is_empty()
            || identity.conversation.len() > 512
            || identity.conversation.chars().any(char::is_control)
            || self.created_at.is_empty()
            || self.updated_at.is_empty()
            || self.approved_at.is_some() != approved
            || (self.phase == AgentAccessPhase::Pending && self.decision.is_some())
            || (self.phase == AgentAccessPhase::Denied
                && !matches!(self.decision, Some(AgentAccessDecision::Denied { .. })))
            || (!matches!(
                self.phase,
                AgentAccessPhase::Pending | AgentAccessPhase::Denied | AgentAccessPhase::Failed
            ) && !approved)
            || (self.phase.terminal() && self.lease.is_some())
            || self.lease.as_ref().is_some_and(|lease| {
                lease.id != self.id
                    || lease.token.is_empty()
                    || lease.holder.is_empty()
                    || lease.expires_at <= 0
            })
            || matches!(
                self.phase,
                AgentAccessPhase::Pausing
                    | AgentAccessPhase::Granting
                    | AgentAccessPhase::Reviving
                    | AgentAccessPhase::Notifying
                    | AgentAccessPhase::Succeeded
            ) && self.progress.hot_update.is_none()
                && self.progress.pause_operation_id.is_none()
            || matches!(
                self.phase,
                AgentAccessPhase::Updating
                    | AgentAccessPhase::Reviving
                    | AgentAccessPhase::Notifying
                    | AgentAccessPhase::Succeeded
            ) && self.progress.grant_generation_floor.is_none()
            || matches!(
                self.phase,
                AgentAccessPhase::Notifying | AgentAccessPhase::Succeeded
            ) && self.progress.hot_update.is_none()
                && self.progress.revive_operation_id.is_none()
            || matches!(
                self.phase,
                AgentAccessPhase::Notifying | AgentAccessPhase::Succeeded
            ) && self.progress.delivered_credential.is_none()
            || matches!(
                self.phase,
                AgentAccessPhase::Notifying | AgentAccessPhase::Succeeded
            ) && self.progress.delivered_runtime.is_none()
            || self.phase == AgentAccessPhase::Succeeded
                && self.progress.notification_command_id.is_none()
            || self
                .progress
                .notification_intent
                .as_ref()
                .is_some_and(|intent| {
                    self.progress.hot_update.is_none()
                        || !matches!(
                            self.phase,
                            AgentAccessPhase::Notifying
                                | AgentAccessPhase::Succeeded
                                | AgentAccessPhase::Failed
                        )
                        || intent.agent_revision == 0
                        || intent.agent_revision > i64::MAX as u64
                        || intent.attempt > 64
                        || intent
                            .target_turn
                            .is_some_and(|turn| turn == 0 || turn > i64::MAX as u64)
                        || intent.previous_commands.len() > intent.attempt as usize
                        || intent.previous_commands.iter().any(|id| {
                            id.as_str().is_empty()
                                || Some(id) == self.progress.notification_command_id.as_ref()
                        })
                        || intent
                            .previous_commands
                            .iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            != intent.previous_commands.len()
                })
            || self.phase == AgentAccessPhase::Updating && self.progress.hot_update.is_none()
            || self.progress.hot_update.as_ref().is_some_and(|hot| {
                !matches!(
                    self.phase,
                    AgentAccessPhase::Updating
                        | AgentAccessPhase::Notifying
                        | AgentAccessPhase::Succeeded
                        | AgentAccessPhase::Failed
                ) || self.progress.pause_operation_id.is_some()
                    || self.progress.revive_operation_id.is_some()
                    || self.progress.grant_generation_floor.is_none()
                    || hot.lifecycle_next_authority == 0
                    || hot.runtime.provision_generation.is_none()
                    || self
                        .progress
                        .delivered_runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime != &hot.runtime)
            })
        {
            return Err(AgentAccessError::Invalid);
        }
        if let Some(AgentAccessDecision::Approved {
            services,
            authorization,
            command_key_hash,
            connection,
            connection_binding,
        }) = &self.decision
        {
            if connection
                .as_ref()
                .is_some_and(|selection| !selection.matches_scope(services))
            {
                return Err(AgentAccessError::Invalid);
            }
            if connection_binding.as_ref().is_some_and(|binding| {
                connection.is_none()
                    || binding.sandbox_id != self.requester.sandbox_id
                    || Some(binding.scope_revision) != self.expected_scope_revision
            }) {
                return Err(AgentAccessError::Invalid);
            }
            let services = services.iter().map(ToString::to_string).collect::<Vec<_>>();
            layer2_core::sandbox::agent_access::validate_access_services(&services)?;
            if command_key_hash.len() != 64
                || !valid_owner_authorization(authorization)
                || !self
                    .proposal
                    .services
                    .iter()
                    .all(|value| services.contains(value))
                || services.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(AgentAccessError::Invalid);
            }
        }
        Ok(())
    }
}

pub(in crate::store) fn read(value: &Document) -> Result<AgentAccessRecord, AgentAccessError> {
    let record: AgentAccessRecord =
        read_record(value).map_err(|_| AgentAccessError::Unavailable)?;
    record
        .validate()
        .map_err(|_| AgentAccessError::Unavailable)?;
    if value.get_str("_id").ok() != Some(&record.id) {
        return Err(AgentAccessError::Unavailable);
    }
    Ok(record)
}
pub(in crate::store) fn document(record: &AgentAccessRecord) -> Result<Document, AgentAccessError> {
    record.validate()?;
    Ok(
        doc! {"_id": &record.id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "record": record_bson(record).map_err(|_| AgentAccessError::Unavailable)?},
    )
}
pub(in crate::store) fn lease_filter(lease: &AgentAccessLease, now: i64) -> Document {
    doc! {"_id": &lease.id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
    "record.lease.id": &lease.id, "record.lease.token": &lease.token,
    "record.lease.holder": &lease.holder, "record.lease.expires_at": {"$gt": now}}
}
pub(super) fn request_id(
    identity: &AgentAccessRequester,
    key: &str,
) -> Result<String, AgentAccessError> {
    Ok(format!(
        "aar_{}",
        digest(&("agent-access-request-v1", &identity.creation, key))?
    ))
}
pub(super) fn digest(value: &impl Serialize) -> Result<String, AgentAccessError> {
    layer2_core::profiles::content_digest(value).map_err(|_| AgentAccessError::Unavailable)
}
