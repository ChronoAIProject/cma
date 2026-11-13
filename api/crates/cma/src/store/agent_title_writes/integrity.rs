//! Title discovery keeps complete validation while maintaining append metadata.
use super::*;

pub(super) async fn transaction_log(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<LogSummary, ApplyError> {
    let mut entries = Vec::new();
    let summary = scan_complete_log_with_session(
        ScanCaller::TitleDiscovery,
        store,
        &agent.id,
        agent.conversation.last_seq,
        session,
        |entry| {
            entries.push(entry);
            Ok(())
        },
    )
    .await
    .map_err(map_transaction_log)?;
    Ok(summary.with_semantic(
        crate::store::agent_log_integrity::audit_checkpoint(store, agent, &entries, session)
            .await
            .map_err(map_transaction_log)?,
    ))
}
