use super::*;

impl HostedAdapter {
    pub(super) async fn exchange(
        &self,
        handle: &RuntimeHandle,
        frame: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let operation = frame.operation();
        let response = self.backend.exchange(handle, frame).await?;
        if let ResponseFrame::Failure(failure) = response {
            if failure.operation != operation {
                return Err(AdapterError::unexpected_reply(operation_name(operation)));
            }
            return Err(failure_error(failure));
        }
        if response_operation(&response) != operation {
            return Err(AdapterError::unexpected_reply(operation_name(operation)));
        }
        Ok(response)
    }

    pub(super) async fn validate_options(
        &self,
        handle: &RuntimeHandle,
        options: &crate::RuntimeOptions,
    ) -> crate::Result<()> {
        let state = self.known_state(handle).await?;
        if options.own_credential != state.own_credential {
            return Err(AdapterError::Rejected(
                "agent credential mode differs from its sandbox".into(),
            ));
        }
        if options.slot.0.trim().is_empty()
            || !options.working_directory.starts_with('/')
            || !options.home_directory.starts_with('/')
        {
            return Err(AdapterError::Rejected(
                "agent slot, working directory, and home directory are required".into(),
            ));
        }
        Ok(())
    }
}
