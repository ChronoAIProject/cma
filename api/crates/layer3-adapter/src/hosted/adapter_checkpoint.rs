use super::*;

pub(super) enum HoldPurpose {
    Capture,
    VerifyPublished,
}

impl HostedAdapter {
    pub(super) async fn checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointHoldRequest,
        purpose: HoldPurpose,
    ) -> crate::Result<CheckpointHoldReceipt> {
        self.known_state(handle).await?;
        if !request.has_valid_writer_resume_token() {
            return Err(AdapterError::Rejected(
                "checkpoint writer resume token must be 64 lowercase hexadecimal characters".into(),
            ));
        }

        let operation = crate::wire::WireOperation::CheckpointForTeardown;
        let response = match purpose {
            HoldPurpose::Capture => {
                self.backend
                    .exchange(handle, RequestFrame::CheckpointForTeardown(request.clone()))
                    .await
            }
            HoldPurpose::VerifyPublished => {
                self.backend.verify_checkpoint_hold(handle, request).await
            }
        };
        let response = match response {
            Ok(ResponseFrame::Failure(failure)) if failure.operation == operation => {
                return Err(failure_error(failure));
            }
            Ok(response) if response_operation(&response) == operation => Ok(response),
            Ok(_) => Err(AdapterError::unexpected_reply(operation_name(operation))),
            Err(error) if !error.is_runtime_loss() => return Err(error),
            Err(error) => Err(error),
        };

        let result = match response {
            Ok(ResponseFrame::CheckpointForTeardown(receipt))
                if receipt.checkpoint.epoch == request.checkpoint.epoch
                    && valid_checkpoint_reference(
                        &receipt.checkpoint.object_reference,
                        &receipt.checkpoint.sha256,
                    )
                    && receipt.checkpoint.writers == WriterState::Held
                    && crate::writer_resume_tokens_match(
                        &receipt.writer_resume_token,
                        &request.writer_resume_token,
                    ) =>
            {
                return Ok(receipt);
            }
            Ok(ResponseFrame::CheckpointForTeardown(_)) => {
                AdapterError::Rejected("runtime returned an invalid held checkpoint receipt".into())
            }
            Ok(_) => unreachable!("response operation was verified"),
            Err(error) => error,
        };

        Err(self
            .recover_checkpoint_writer_hold(handle, &request.writer_resume_token, result)
            .await)
    }
}
