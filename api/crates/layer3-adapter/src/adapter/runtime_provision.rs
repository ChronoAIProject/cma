//! Default provisioning preserves the legacy first-generation contract.
use crate::{AdapterError, Result, RuntimeHandle, RuntimeProvisionRequest, SandboxAdapter};

pub(super) async fn provision<A: SandboxAdapter + ?Sized>(
    adapter: &A,
    request: &RuntimeProvisionRequest,
) -> Result<RuntimeHandle> {
    if request.funded_until.is_some() {
        return Err(super::unsupported(
            "funded provider creation is unavailable",
        ));
    }
    if request.generation == 0 {
        return Err(AdapterError::Rejected(
            "runtime generation must be non-zero".into(),
        ));
    }
    if request.generation == 1 {
        return adapter.provision(&request.spec).await;
    }
    Err(super::unsupported(
        "generation-specific provisioning is not supported",
    ))
}
