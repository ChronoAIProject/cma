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
        match self.exchange(handle, RequestFrame::Credential(frame)).await {
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
