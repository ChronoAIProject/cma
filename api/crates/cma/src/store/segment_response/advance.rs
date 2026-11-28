use super::*;
use crate::store::agent_log_reads::integrity::ReadHead;
use layer2_core::sandbox::AgentLogEntry;
use mongodb::ClientSession;

pub(super) fn may_have_advanced(before: &Agent, after: &Agent, turn: Option<u64>) -> bool {
    turn.is_some()
        && before.conversation.active_turn == turn
        && before.revision.checked_add(1) == Some(after.revision)
        && matches!(
            after
                .conversation
                .last_seq
                .checked_sub(before.conversation.last_seq),
            Some(1..=32)
        )
}

pub(super) fn classify(
    before: &Agent,
    after: &Agent,
    turn: Option<u64>,
    rows: &[AgentLogEntry],
) -> SandboxStoreError {
    if turn.is_some_and(|turn| before.proves_output_advance(after, turn, rows)) {
        SandboxStoreError::CaptureAdvanced
    } else {
        SandboxStoreError::SequenceConflict
    }
}

pub(super) async fn legacy(
    store: &MongoSandboxStore,
    before: &Agent,
    after: &Agent,
    head: &ReadHead,
    turn: Option<u64>,
    tail: Option<&AgentLogEntry>,
    session: &mut ClientSession,
) -> Result<SandboxStoreError, SandboxStoreError> {
    if !head.trusted || !may_have_advanced(before, after, turn) {
        return Ok(SandboxStoreError::SequenceConflict);
    }
    if let Some(tail) = tail {
        let original = read_bounded(
            store,
            &before.id,
            tail.seq - 1,
            tail.seq,
            head,
            session,
            MAX_BYTES,
        )
        .await?;
        if original.as_deref() != Some(std::slice::from_ref(tail)) {
            return Err(SandboxStoreError::Corrupt(
                "retained output boundary changed".into(),
            ));
        }
    }
    let rows = read_bounded(
        store,
        &before.id,
        before.conversation.last_seq,
        head.last_seq,
        head,
        session,
        MAX_BYTES,
    )
    .await?;
    Ok(classify(
        before,
        after,
        turn,
        rows.as_deref().unwrap_or_default(),
    ))
}
