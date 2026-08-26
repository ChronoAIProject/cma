//! Installing opaque authenticated policy on one exact live runtime.

use crate::{CoreError, SessionId, SessionService};

impl SessionService {
    /// Build policy against the current runtime audience, deliver it while
    /// lifecycle replacement is fenced, and return the exact acknowledgement.
    ///
    /// The closure keeps provider-shaped signing outside Layer 2. The core
    /// owns only the ordering invariant: the runtime named in the signed data
    /// is the same runtime that receives it, for the whole operation.
    pub async fn install_runtime_policy<F>(
        &self,
        id: &SessionId,
        build: F,
    ) -> Result<(String, layer3_adapter::RuntimePolicyReceipt), CoreError>
    where
        F: FnOnce(&str) -> Result<String, CoreError> + Send,
    {
        let _lifecycle = self.lock_lifecycle(id).await;
        let (runtime, kernel) = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no live runtime entry".into())
            })?;
            if entry.trigger_origin.is_none() {
                return Err(CoreError::InvalidState);
            }
            let runtime = entry.runtime.clone().ok_or_else(|| {
                CoreError::RuntimeUnavailable("the session has no current runtime".into())
            })?;
            (runtime, entry.session.kernel.clone())
        };
        let adapter = self.adapters.get(&kernel).ok_or_else(|| {
            CoreError::CapabilityUnsupported("authenticated_runtime_policy".into())
        })?;
        let envelope = build(&runtime.0)?;
        let receipt = adapter
            .install_runtime_policy(&runtime, &envelope)
            .await
            .map_err(|error| match error {
                layer3_adapter::AdapterError::Unavailable(detail) => {
                    CoreError::RuntimeUnavailable(detail)
                }
                layer3_adapter::AdapterError::Rejected(detail)
                | layer3_adapter::AdapterError::AtCapacity(detail) => {
                    CoreError::RuntimeRefused(detail)
                }
                layer3_adapter::AdapterError::Unsupported(_) => {
                    CoreError::CapabilityUnsupported("authenticated_runtime_policy".into())
                }
            })?;
        Ok((runtime.0, receipt))
    }
}
