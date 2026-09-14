use super::*;

impl HostedAdapter {
    pub(super) async fn resume_checkpoint_writer_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointWriterResumeRequest,
    ) -> crate::Result<CheckpointWriterResumeReceipt> {
        match self
            .exchange(
                handle,
                RequestFrame::ResumeCheckpointWriters(request.clone()),
            )
            .await?
        {
            ResponseFrame::ResumeCheckpointWriters(receipt)
                if receipt.writers_resumed
                    && crate::writer_resume_tokens_match(
                        &receipt.writer_resume_token,
                        &request.writer_resume_token,
                    ) =>
            {
                Ok(receipt)
            }
            ResponseFrame::ResumeCheckpointWriters(_) => Err(AdapterError::Rejected(
                "runtime did not acknowledge the exact checkpoint writer resume".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    pub(super) async fn recover_checkpoint_writer_hold(
        &self,
        handle: &RuntimeHandle,
        writer_resume_token: &str,
        original: AdapterError,
    ) -> AdapterError {
        let resume = CheckpointWriterResumeRequest {
            writer_resume_token: writer_resume_token.into(),
        };
        match self.resume_checkpoint_writer_hold(handle, &resume).await {
            Ok(_) => original,
            Err(recovery) => AdapterError::Unavailable(format!(
                "checkpoint writer recovery could not be verified after {original}; recovery failed: {recovery}"
            )),
        }
    }
}
