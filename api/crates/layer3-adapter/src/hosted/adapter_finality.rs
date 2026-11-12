//! Exact finality checks shared by the hosted adapter lifecycle dispatch.
use super::*;

impl HostedAdapter {
    pub(super) async fn finalize_runtime_exact(
        &self,
        original: &crate::RuntimeProvisionReceipt,
        operation_id: &str,
    ) -> crate::Result<crate::RuntimeFinalityReceipt> {
        let receipt = self
            .backend
            .finalize_runtime(original, operation_id)
            .await?;
        receipt.validate()?;
        if receipt.original() != original || receipt.delete_operation_id() != operation_id {
            return Err(AdapterError::Rejected(
                "provider returned another runtime finality receipt".into(),
            ));
        }
        self.states.lock().await.remove(original.handle());
        Ok(receipt)
    }

    pub(super) async fn teardown_retained_exact(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<()> {
        self.backend.teardown_retained(handle).await?;
        self.states.lock().await.remove(handle);
        Ok(())
    }
}
