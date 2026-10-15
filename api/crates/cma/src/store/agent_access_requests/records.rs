use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_access::{AgentAccessError, AgentAccessRequest, AgentAccessStatus};
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::{RuntimeBinding, SandboxCredentialAuthority};
use layer2_core::ServiceId;
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentAccessPhase {
    Pending,
    Connecting,
    WaitingQuiet,
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
            Self::WaitingQuiet | Self::Pausing | Self::Granting | Self::Reviving | Self::Notifying
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
        authorization: AgentAuthorizationRevision,
    },
    Denied {
        command_key_hash: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessLease {
    pub id: String,
    pub token: String,
    pub holder: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessProgress {
    pub pause_operation_id: Option<String>,
    pub grant_generation_floor: Option<u64>,
    pub revive_operation_id: Option<String>,
    pub delivered_credential: Option<SandboxCredentialAuthority>,
    pub delivered_runtime: Option<RuntimeBinding>,
    pub notification_command_id: Option<AgentCommandId>,
}
impl AgentAccessProgress {
    pub(super) fn extends(&self, previous: &Self) -> bool {
        (previous.pause_operation_id.is_none()
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessRecord {
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
        }
    }

    pub(crate) fn approved_services(&self) -> Result<&[ServiceId], AgentAccessError> {
        match &self.decision {
            Some(AgentAccessDecision::Approved { services, .. }) => Ok(services),
            _ => Err(AgentAccessError::Conflict),
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
            ) && self.progress.pause_operation_id.is_none()
            || matches!(
                self.phase,
                AgentAccessPhase::Reviving
                    | AgentAccessPhase::Notifying
                    | AgentAccessPhase::Succeeded
            ) && self.progress.grant_generation_floor.is_none()
            || matches!(
                self.phase,
                AgentAccessPhase::Notifying | AgentAccessPhase::Succeeded
            ) && self.progress.revive_operation_id.is_none()
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
        {
            return Err(AgentAccessError::Invalid);
        }
        if let Some(AgentAccessDecision::Approved {
            services,
            authorization,
            command_key_hash,
        }) = &self.decision
        {
            let services = services.iter().map(ToString::to_string).collect::<Vec<_>>();
            layer2_core::sandbox::agent_access::validate_access_services(&services)?;
            if command_key_hash.len() != 64
                || !authorization.authority_id.starts_with("personal:")
                || authorization.authority_id.len() <= "personal:".len()
                || authorization.authority_id.chars().any(char::is_control)
                || authorization.policy_revision == 0
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
