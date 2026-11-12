//! Server-created, parent-bound input provenance. Source adapters own the opaque
//! proof schema and revalidate it; ordinary client messages cannot provide it.
use super::*;
use crate::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::{CallerContext, ResourceCapability, ResourceTarget},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalEventContinuation {
    pub parent_command_id: AgentCommandId,
    pub parent_response_id: AgentResponseId,
    pub parent_fingerprint: String,
    /// Actual authorization revision of the original Manager command.
    pub binding_generation: u64,
    /// Separately verified optional generation of the external source request.
    pub source_generation: Option<u64>,
    pub source_expires_at: i64,
    /// Stable source-defined artifact slot; changed content cannot mint a slot.
    pub artifact_id: String,
    pub source_proof: serde_json::Value,
}
impl ExternalEventContinuation {
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        if self.parent_command_id.as_str().is_empty()
            || self.parent_response_id.as_str().is_empty()
            || self.parent_fingerprint.len() != 64
            || !self
                .parent_fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.binding_generation == 0
            || self.source_generation == Some(0)
            || self.source_expires_at <= 0
            || self.artifact_id.is_empty()
            || self.artifact_id.len() > 128
            || self.artifact_id.chars().any(char::is_control)
            || !self.source_proof.is_object()
            || serde_json::to_vec(&self.source_proof).map_or(true, |v| v.len() > 16_384)
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }
    pub fn key(&self) -> String {
        format!(
            "external-supplement-v1:{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&self.parent_command_id, &self.artifact_id))
                    .expect("typed identity")
            )
        )
    }
    pub fn require_parent(
        &self,
        envelope: &AgentCommandEnvelope,
        receipt: &AdmissionReceipt,
    ) -> Result<(), CommandLedgerError> {
        self.validate()?;
        let parent = &receipt.envelope;
        parent
            .validate_integrity()
            .map_err(|_| CommandLedgerError::Corrupt)?;
        if parent.continuation.is_some()
            || parent.command.external_event().is_none()
            || receipt.response_id.as_ref() != Some(&self.parent_response_id)
            || self.parent_command_id != parent.command_id
            || self.parent_fingerprint != parent.request_fingerprint
            || parent.authorization.grant_revision != Some(self.binding_generation)
            || !parent.authorization.authority_id.starts_with("binding:")
            || envelope.authorization != parent.authorization
            || envelope.agent_id != parent.agent_id
            || envelope.owner != parent.owner
            || envelope.actor != parent.actor
            || envelope.source.kind != AgentSourceKind::Api
            || !matches!(&envelope.command, AgentCommand::Message { input, order: AgentInputOrder::Append, metadata }
                if input.external_event().is_none() && metadata.is_empty())
        {
            return Err(CommandLedgerError::Invalid);
        }
        Ok(())
    }
}

/// Live, nonserialized source authority, composed by the application adapter.
/// The ordered runtime writer invokes it again immediately before input IO.
#[async_trait::async_trait]
pub trait CommandSourceGuard: Send + Sync {
    async fn check(&self, envelope: &AgentCommandEnvelope)
        -> Result<(), CommandAuthorizationError>;
}

impl AgentCommandService {
    /// This separate server interface cannot accept client command metadata as
    /// authority. Store adapters also verify the exact parent in their transaction.
    pub async fn admit_external_continuation(
        &self,
        context: &CallerContext,
        credential: &str,
        continuation: ExternalEventContinuation,
        text: String,
        source: &dyn CommandSourceGuard,
    ) -> Result<AgentResponseReading, CommandAdmissionError> {
        continuation.validate()?;
        let parent = self
            .store
            .get_command(&continuation.parent_command_id)
            .await?
            .ok_or(CommandLedgerError::NotFound)?;
        parent.validate()?;
        let (authorized, mut request) = self
            .prepare(
                context,
                credential,
                CommandSubmissionSource::Api,
                CommandSubmission {
                    agent_id: parent.receipt.envelope.agent_id.clone(),
                    command: AgentCommand::message(text),
                    idempotency_key: continuation.key(),
                    expected_agent_revision: None,
                    requested_model: None,
                },
            )
            .await?;
        continuation.require_parent(&request.envelope, &parent.receipt)?;
        let now = SystemTime::now();
        let expiry = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(continuation.source_expires_at as u64))
            .ok_or(CommandLedgerError::Invalid)?;
        if expiry <= now {
            return Err(CommandAuthorizationError::Revoked.into());
        }
        let admitted =
            crate::seconds_from_rfc3339(&request.admitted_at).ok_or(CommandLedgerError::Corrupt)?;
        request.queue_deadline = crate::rfc3339_at(
            expiry.min(SystemTime::UNIX_EPOCH + Duration::from_secs(admitted + 86_400)),
        );
        request.envelope.continuation = Some(continuation);
        let target = ResourceTarget::Agent {
            agent_id: request.envelope.agent_id.clone(),
        };
        source.check(&request.envelope).await?;
        let result = self
            .with_authority_bound(
                &authorized,
                ResourceCapability::Interact,
                self.store.reserve_command(request.clone()),
            )
            .await;
        self.fence(&authorized, &target, ResourceCapability::Interact)
            .await?;
        source.check(&request.envelope).await?;
        let record = match result?? {
            CommandReservationResult::Created(r) | CommandReservationResult::Existing(r) => *r,
        };
        let CommandReservationResult::Existing(record) = request.replay(record)? else {
            return Err(CommandLedgerError::Corrupt.into());
        };
        Ok(AgentResponseReading {
            deferred_failure_detail: None,
            command: CommandStatusView::from_record(&record),
            metadata: record.receipt.envelope.response_metadata(),
            nyxid_trace: record.work.nyxid_trace,
        })
    }
}

impl AgentCommandEnvelope {
    /// Server provenance takes precedence over user-supplied message metadata.
    pub fn response_metadata(&self) -> std::collections::BTreeMap<String, String> {
        if let Some(proof) = &self.continuation {
            return [
                ("cma_parent_command_id", proof.parent_command_id.to_string()),
                (
                    "cma_parent_response_id",
                    proof.parent_response_id.to_string(),
                ),
                ("cma_parent_fingerprint", proof.parent_fingerprint.clone()),
                ("cma_artifact_id", proof.artifact_id.clone()),
                ("cma_command_fingerprint", self.request_fingerprint.clone()),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect();
        }
        match &self.command {
            AgentCommand::Message { metadata, .. } => metadata.clone(),
            _ => Default::default(),
        }
    }
}
