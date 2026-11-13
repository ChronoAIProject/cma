use super::*;

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
        ReplayRead::Snapshot => {
            scan_complete_log_with_snapshot_session(
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
    }
    Ok(initial_log)
}
