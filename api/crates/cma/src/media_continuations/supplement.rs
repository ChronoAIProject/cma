use super::protocol::{sha256, supplement_text, MediaArtifactClaim};
use super::{MediaContinuations, MediaUploadRecord};
use layer1_public::{
    rest::binding_media::{BindingMediaError as Error, BindingMediaPath, BindingMediaSupplement},
    AuthenticatedResourceCaller,
};
use layer2_core::sandbox::agent_io::{
    AgentResponseReading, CommandAdmissionError, CommandLedgerError, ExternalEventContinuation,
};
impl MediaContinuations {
    pub(super) async fn admit_supplement(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        body: BindingMediaSupplement,
    ) -> Result<AgentResponseReading, Error> {
        let (parent, _) = self.parent_for_caller(&caller, &path).await?;
        let parent_response_id = parent.receipt.response_id.clone().ok_or(Error::Forbidden)?;
        let parent = parent.receipt.envelope;
        let artifact: MediaArtifactClaim =
            serde_json::from_value(body.artifact).map_err(|_| Error::Invalid)?;
        artifact
            .validate_derived_text(&body.derived_text)
            .map_err(|_| Error::Invalid)?;
        if artifact.attachment_index != path.attachment_index {
            return Err(Error::Invalid);
        }
        let record = self
            .uploads
            .get_media_upload(&MediaUploadRecord::id(
                &parent.command_id,
                path.attachment_index,
            ))
            .await?
            .ok_or(Error::NotReady)?;
        record.validate()?;
        let receipt = record.receipt.as_ref().ok_or(Error::NotReady)?;
        if record.artifact != artifact.original()
            || record.source_generation != body.gateway_binding_generation
            || record.source_expires_at != body.event_expires_at
            || record.parent_fingerprint != parent.request_fingerprint
        {
            return Err(Error::Conflict);
        }
        let text = supplement_text(&artifact, &receipt.path, &body.derived_text)
            .map_err(|_| Error::Invalid)?;
        if artifact.text_sha256.as_deref() != Some(sha256(text.as_bytes()).as_str()) {
            return Err(Error::Conflict);
        }
        let proof = ExternalEventContinuation {
            parent_response_id,
            parent_command_id: parent.command_id,
            parent_fingerprint: parent.request_fingerprint,
            binding_generation: parent
                .authorization
                .grant_revision
                .ok_or(Error::Forbidden)?,
            source_generation: body.gateway_binding_generation,
            source_expires_at: body.event_expires_at,
            artifact_id: format!("{}:{}", artifact.attachment_index, artifact.kind.as_str()),
            source_proof: serde_json::to_value(&artifact).map_err(|_| Error::Invalid)?,
        };
        self.commands
            .admit_external_continuation(
                &caller.context,
                caller.credential.as_ref().ok_or(Error::Forbidden)?.value(),
                proof,
                text,
                self,
            )
            .await
            .map_err(|e| match e {
                CommandAdmissionError::Ledger(
                    CommandLedgerError::Unavailable
                    | CommandLedgerError::Corrupt
                    | CommandLedgerError::QueueFull,
                ) => Error::Unavailable,
                CommandAdmissionError::Ledger(
                    CommandLedgerError::IdempotencyConflict | CommandLedgerError::Conflict,
                ) => Error::Conflict,
                CommandAdmissionError::Authority(
                    layer2_core::sandbox::agent_io::CommandAuthorizationError::Unavailable,
                ) => Error::Unavailable,
                CommandAdmissionError::Access(error) => super::parent::resource_error(error),
                CommandAdmissionError::Input(_) => Error::Invalid,
                _ => Error::Forbidden,
            })
    }
}
