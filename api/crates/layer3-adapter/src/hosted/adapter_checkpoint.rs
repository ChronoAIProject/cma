use super::*;

pub(super) enum HoldPurpose {
    Capture,
    VerifyPublished,
}

impl HostedAdapter {
    pub(super) async fn restore_checkpoint(
        &self,
        handle: &RuntimeHandle,
        request: &RestoreRequest,
    ) -> crate::Result<()> {
        self.known_state(handle).await?;
        if !valid_checkpoint_reference(&request.object_reference, &request.sha256) {
            return Err(AdapterError::Rejected(
                "restore object reference or digest is invalid".into(),
            ));
        }
        expect_ack(
            self.exchange(handle, RequestFrame::Restore(request.clone()))
                .await?,
        )
    }

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

impl HostedAdapter {
    pub(super) async fn bootstrap_restore_exact(
        &self,
        handle: &RuntimeHandle,
        request: &RestoreBootstrapRequest,
    ) -> crate::Result<RestoreBootstrapReceipt> {
        let state = self.known_state(handle).await?;
        if request.sandbox_id != state.provision.sandbox_id
            || request.operation_id.trim().is_empty()
            || request.operation_id.len() > 256
            || request.operation_id.chars().any(char::is_control)
            || !request.has_valid_sha256()
            || !request.checkpoint.has_valid_reference()
        {
            return Err(AdapterError::Rejected(
                "restore bootstrap identity is invalid".into(),
            ));
        }
        match self
            .exchange(handle, RequestFrame::BootstrapRestore(request.clone()))
            .await?
        {
            ResponseFrame::BootstrapRestore(receipt)
                if receipt.operation_id == request.operation_id
                    && receipt.request_sha256 == request.request_sha256
                    && receipt.restore_request_sha256 == request.checkpoint.computed_sha256()
                    && receipt.installation_inherited =>
            {
                Ok(receipt)
            }
            ResponseFrame::BootstrapRestore(_) => Err(AdapterError::Rejected(
                "runtime did not acknowledge the exact restore bootstrap".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }

    pub(super) async fn checkpoint_exact(
        &self,
        handle: &RuntimeHandle,
        request: &CheckpointRequest,
    ) -> crate::Result<CheckpointReceipt> {
        self.known_state(handle).await?;
        match self
            .exchange(handle, RequestFrame::Checkpoint(request.clone()))
            .await?
        {
            ResponseFrame::Checkpoint(receipt)
                if receipt.epoch == request.epoch
                    && valid_checkpoint_reference(&receipt.object_reference, &receipt.sha256)
                    && receipt.writers == WriterState::Resumed =>
            {
                Ok(receipt)
            }
            ResponseFrame::Checkpoint(_) => Err(AdapterError::Rejected(
                "runtime returned an invalid checkpoint receipt".into(),
            )),
            _ => unreachable!("exchange verifies response operation"),
        }
    }
}
