use super::*;
use crate::store::agent_log_integrity::{
    scan_complete_log_with_snapshot_policy, IntegrityFailurePolicy,
};

pub(super) async fn legacy_initial(
    store: &MongoSandboxStore,
    agent: &layer2_core::sandbox::Agent,
    session: &mut ClientSession,
    read: ReplayRead,
) -> Result<Option<layer2_core::sandbox::AgentLogEntry>, LoadError> {
    let last_seq = agent.conversation.last_seq;
    let mut initial_log = None;
    match read {
        ReplayRead::Transaction => {
            scan_complete_log_with_session(
                ScanCaller::CommandAdmission,
                store,
                &agent.id,
                last_seq,
                session,
                |entry| {
                    if initial_log.is_none() {
                        initial_log = Some(entry);
                    }
                    Ok(())
                },
            )
            .await?;
        }
        ReplayRead::Snapshot | ReplayRead::UncertainSnapshot => {
            // Proof for an uncertain reuse commit is read-only: refuse corrupt
            // or missing evidence without quarantine or repair writes.
            let (caller, policy) = if matches!(read, ReplayRead::UncertainSnapshot) {
                (
                    ScanCaller::UnknownCommitProof,
                    IntegrityFailurePolicy::ReportOnly,
                )
            } else {
                (
                    ScanCaller::CommandAdmission,
                    IntegrityFailurePolicy::Quarantine,
                )
            };
            scan_complete_log_with_snapshot_policy(
                caller,
                store,
                &agent.id,
                last_seq,
                session,
                policy,
                |entry| {
                    if initial_log.is_none() {
                        initial_log = Some(entry);
                    }
                    Ok(())
                },
            )
            .await?;
        }
    }
    Ok(initial_log)
}
