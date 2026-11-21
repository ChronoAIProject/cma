use super::*;
use layer2_core::sandbox::{Sandbox, SandboxLifecycleBlocker};

pub(super) async fn require(
    store: &MongoSandboxStore,
    current: &Sandbox,
    next: &Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    if store
        .legacy_reuse_blocks_quiescence(current, next, session)
        .await
        .map_err(ApplyError::Driver)?
    {
        return Err(ApplyError::Domain(SandboxStoreError::LifecycleBusy {
            status: current.status,
            reason: SandboxLifecycleBlocker::WorkInProgress,
        }));
    }
    Ok(())
}
