use super::*;

impl HostedAdapter {
    pub(super) async fn upload_attachment(
        &self,
        handle: &RuntimeHandle,
        request: &crate::AgentAttachmentRequest,
        content: Vec<u8>,
    ) -> crate::Result<crate::AgentAttachmentReceipt> {
        request.validate_content(&content)?;
        let state = self.known_state(handle).await?;
        let home = state.home_volume.as_ref().ok_or_else(unsupported)?;
        if !home.request.policy.mounts_full_home() {
            return Err(unsupported());
        }
        if state.provision.sandbox_id != request.sandbox_id
            || state.provision.generation != request.runtime_generation
        {
            return Err(AdapterError::Rejected(
                "attachment runtime identity changed".into(),
            ));
        }
        let status = self.inspect_runtime_status(handle).await?;
        if !status.agent_attachment_upload {
            return Err(unsupported());
        }
        if status.boot_id != request.boot_id || !status.credential_ready || status.restore_pending {
            return Err(AdapterError::Rejected(
                "attachment runtime is not the admitted process".into(),
            ));
        }
        let upload = crate::AgentAttachmentUpload::new(request.clone(), &content)?;
        let response = self
            .exchange(handle, RequestFrame::UploadAgentAttachment(upload))
            .await?;
        let ResponseFrame::UploadAgentAttachment(receipt) = response else {
            return Err(AdapterError::unexpected_reply("upload agent attachment"));
        };
        receipt.validate_for(request)?;
        self.verify_home(home).await?;
        Ok(receipt)
    }
}
fn unsupported() -> AdapterError {
    AdapterError::Unsupported(
        "agent attachments require a current runtime with the complete persistent home".into(),
    )
}
