use super::*;

impl HostedAdapter {
    pub(crate) async fn prove_agent_process_closed(
        &self,
        handle: &RuntimeHandle,
        proof: &crate::wire::AgentProcessCloseProof,
    ) -> crate::Result<bool> {
        self.known_state(handle).await?;
        let request = RequestFrame::ProveAgentProcessClosed(proof.clone());
        let operation = request.operation();
        match self.backend.exchange(handle, request).await? {
            ResponseFrame::ProveAgentProcessClosed(observed) if &observed == proof => Ok(true),
            ResponseFrame::Failure(failure) if failure.operation == operation => {
                if !failure.retryable && failure.code == "agent_process_close_unproved" {
                    Ok(false)
                } else {
                    Err(failure_error(failure))
                }
            }
            _ => Err(AdapterError::Rejected(
                "runtime returned a mismatched Agent Close proof".into(),
            )),
        }
    }

    pub(crate) async fn fence_agent_restart(
        &self,
        handle: &RuntimeHandle,
        request: &crate::wire::AgentRestartFence,
    ) -> crate::Result<()> {
        self.known_state(handle).await?;
        match self
            .exchange(handle, RequestFrame::FenceAgentRestart(request.clone()))
            .await?
        {
            ResponseFrame::FenceAgentRestart(receipt) if &receipt == request => Ok(()),
            _ => Err(AdapterError::Rejected(
                "runtime returned a mismatched Agent restart fence".into(),
            )),
        }
    }

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
