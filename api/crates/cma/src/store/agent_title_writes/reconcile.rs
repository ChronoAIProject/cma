use super::*;

pub(super) async fn reconcile_title(
    store: &MongoSandboxStore,
    receipt: &TitleReceipt,
    snapshot_time: Timestamp,
) -> Result<bool, AgentTitleStoreError> {
    let mut session = snapshot_session(store, Some(snapshot_time)).await?;
    let document = store
        .agents
        .find_one(doc! { "_id": receipt.agent.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|_| AgentTitleStoreError::Unavailable)?;
    let Some(document) = document else {
        return Ok(false);
    };
    let agent = read_agent(&document).map_err(map_store_error)?;
    if agent != receipt.agent {
        return Ok(false);
    }
    let state = read_title_state(&document, &agent)?;
    if state.stored.as_ref() != Some(&receipt.stored) || !receipt.proof.matches(&state) {
        return Ok(false);
    }
    let mut found = None;
    scan_complete_log_with_snapshot_session(
        ScanCaller::UnknownCommitProof,
        store,
        &agent.id,
        agent.conversation.last_seq,
        &mut session,
        |entry| {
            if entry.seq == receipt.entry.seq {
                found = Some(entry);
            }
            Ok(())
        },
    )
    .await
    .map_err(map_reconciliation_log)?;
    match found {
        Some(entry) if entry == receipt.entry => Ok(true),
        Some(_) => Err(AgentTitleStoreError::SequenceConflict),
        None => Ok(false),
    }
}

impl CompletionProof {
    fn matches(&self, state: &TitleState) -> bool {
        match self {
            Self::Automatic(claim) => state.completed_by(claim),
            Self::User(token) => state.user_commit_token.as_ref() == Some(token),
        }
    }
}
