//! Validate the credential operation against its original retained home.
use super::*;

impl HostedAdapter {
    pub(super) async fn deliver_credential_exact(
        &self,
        handle: &RuntimeHandle,
        frame: CredentialFrame,
    ) -> crate::Result<()> {
        if frame.operation_id.trim().is_empty()
            || frame.value.expose().is_empty()
            || frame.custody_generation == Some(0)
        {
            return Err(AdapterError::Rejected(
                "credential operation id, value, and non-zero generation are required".into(),
            ));
        }
        self.verify_bound_home(handle).await?;
        let operation_id = frame.operation_id.clone();
        let request = if frame.github_route.is_some() {
            let status = self.inspect_runtime_status(handle).await?;
            if !status.exact_github_credential_route {
                return Err(AdapterError::Unsupported(
                    "runtime does not support a bound GitHub credential route".into(),
                ));
            }
            if frame
                .github_route
                .as_ref()
                .is_some_and(|route| route.repository_scope.is_some())
                && !status.github_repository_scope
            {
                return Err(AdapterError::Unsupported(
                    "runtime does not support GitHub repository scope reporting".into(),
                ));
            }
            RequestFrame::CredentialForReceiver {
                receiver: crate::RuntimeStatusRequest {
                    sandbox_id: status.sandbox_id,
                    generation: status.generation,
                    home_identity: status.home_identity,
                },
                boot_id: status.boot_id,
                credential: frame,
            }
        } else {
            RequestFrame::Credential(frame)
        };
        match self.exchange(handle, request).await {
            Ok(ResponseFrame::Credential(CredentialReceipt {
                operation_id: returned,
                installed: true,
            })) if returned == operation_id => Ok(()),
            Ok(ResponseFrame::Credential(_)) => Err(AdapterError::Rejected(
                "runtime did not acknowledge the exact credential operation".into(),
            )),
            Ok(_) => unreachable!("exchange verifies response operation"),
            Err(error) => Err(error),
        }
    }
}
