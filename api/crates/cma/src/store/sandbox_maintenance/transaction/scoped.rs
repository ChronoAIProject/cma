use super::*;
use crate::store::{
    agent_capacity_recovery::source::sandbox_uses_segments,
    agent_log_segments::transaction_scope::TransactionScope, sandbox_mongo::start_transaction,
};

pub(in crate::store::sandbox_maintenance) async fn attempt(
    store: &MongoSandboxStore,
    request: &MaintenanceRequest,
    session: &mut ClientSession,
) -> Result<MaintenanceReceipt, ApplyError> {
    if !matches!(request, MaintenanceRequest::Inactivity(_))
        || !sandbox_uses_segments(store, request.sandbox_id()).await?
    {
        start_transaction(session)
            .await
            .map_err(ApplyError::Driver)?;
        return apply(store, request, session).await;
    }
    let mut scope = TransactionScope::start(session)
        .await
        .map_err(ApplyError::Driver)?;
    let sources = CapturedActivities::capture(store, request.sandbox_id(), &mut scope).await?;
    // No capture escapes this function or crosses a transaction boundary. The
    // original domain checks and actual child/Sandbox fences follow in the same
    // transaction before caller-controlled commit, abort or unknown handling.
    apply_with_sources(store, request, scope.release(), Some(&sources)).await
}
