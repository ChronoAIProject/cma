//! The linear-message preflight shares the transactional dispatch proof.
use super::*;
use layer2_core::sandbox::Agent;

pub(super) async fn read(
    store: &MongoSandboxStore,
    expected: &Agent,
) -> Result<Option<u64>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let row = store
        .agents
        .find_one(doc! { "_id": expected.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .ok_or(SandboxStoreError::NotFound)?;
    let agent = read_agent(&row)?;
    if &agent != expected {
        return Err(SandboxStoreError::Conflict);
    }
    let proof = trusted_summary(
        store,
        &agent,
        &row,
        ScanCaller::CommandAdmission,
        &mut session,
        true,
    )
    .await
    .map_err(map_error)?;
    if let Some(proof) = proof {
        return Ok(proof.semantic().and_then(SemanticCheckpoint::latest_turn));
    }
    let mut latest = None;
    scan_complete_log_with_snapshot_session(
        ScanCaller::CommandAdmission,
        store,
        &agent.id,
        agent.conversation.last_seq,
        &mut session,
        |entry| {
            if let AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                if turn_sequence != entry.seq {
                    return Err(SandboxStoreError::SequenceConflict);
                }
                latest = Some(turn_sequence);
            }
            Ok(())
        },
    )
    .await
    .map_err(map_error)?;
    Ok(latest)
}

fn map_error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
