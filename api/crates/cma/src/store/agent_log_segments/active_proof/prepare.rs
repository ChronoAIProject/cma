//! Explicit one-Agent preparation under the same snapshot/majority write fence.
use super::*;
use crate::store::{
    agent_log::read_entry,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogKind};
use mongodb::{options::Hint, Client, Database};

const fn unavailable() -> &'static str {
    "active log proof could not establish the original current head"
}

pub(in crate::store) async fn prepare(
    client: &Client,
    database: &Database,
    agent_id: &str,
) -> Result<i64, &'static str> {
    let mut session = client.start_session().await.map_err(|_| unavailable())?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable())?;
    let proof = match prepare_transaction(database, agent_id, &mut session).await {
        Ok(proof) => proof,
        Err(_) => {
            let _ = session.abort_guarded().await;
            return Err(unavailable());
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed | CommitOutcome::Unknown(_) => {}
        _ => return Err(unavailable()),
    }
    // A concurrent predecessor writer may have advanced the head. Report
    // success only if the exact proof is still current in one fresh snapshot.
    let mut snapshot = client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|_| unavailable())?;
    let (agent, head) = current(database, agent_id, &mut snapshot, true)
        .await
        .map_err(|_| unavailable())?;
    if head != proof.head
        || i64::try_from(agent.revision).ok() != Some(proof.agent_revision)
        || read_database(database, &head, agent.revision, &mut snapshot, true)
            .await
            .map_err(|_| unavailable())?
            != Some(proof.leaves)
    {
        return Err(unavailable());
    }
    Ok(head.revision)
}

async fn prepare_transaction(
    database: &Database,
    agent_id: &str,
    session: &mut ClientSession,
) -> Result<ActiveProof, TransactionLogError> {
    let (agent, head) = current(database, agent_id, session, false).await?;
    let existing = read_database(database, &head, agent.revision, session, false).await?;
    let leaves = scan(database, &agent, &head, session).await?;
    let proof = ActiveProof::new(&head, agent.revision, leaves)?;
    if let Some(existing) = existing {
        if existing != proof.leaves {
            return Err(corrupt().into());
        }
        return Ok(proof);
    }
    // Exact no-op replacement participates in the same head write as every
    // publication. A competing writer must conflict or make readback stale.
    let heads = majority_collection::<Document>(database, super::super::HEADS);
    let canonical_head = bson::serialize_to_document(&head).map_err(|_| corrupt())?;
    if heads
        .replace_one(canonical_head.clone(), canonical_head)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .matched_count
        != 1
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    let agents = majority_collection::<Document>(database, "agents");
    let revision = i64::try_from(agent.revision).map_err(|_| corrupt())?;
    if agents
        .update_one(
            doc! {"_id":agent_id,"revision":revision,
            "agent_log_incarnation":&head.incarnation,"agent_log_head_revision":head.revision},
            doc! {"$set":{"agent_log_head_revision":head.revision}},
        )
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .matched_count
        != 1
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    proof.persist_database(database, session).await?;
    Ok(proof)
}

async fn current(
    database: &Database,
    agent_id: &str,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(Agent, Head), TransactionLogError> {
    let agents = majority_collection::<Document>(database, "agents");
    let row = crate::store::stale_turns::find_with_proof_budget(
        &agents,
        doc! {"_id":agent_id},
        session,
        snapshot,
    )
    .await
    .map_err(TransactionLogError::Driver)?
    .ok_or(SandboxStoreError::NotFound)?;
    let agent = crate::store::agents::read_agent(&row)?;
    let head = super::super::reads::metadata::read_head_from(
        database, &row, session, snapshot, None, None,
    )
    .await?;
    let projections = majority_collection::<Document>(database, super::super::PROJECTIONS);
    let projection = crate::store::stale_turns::find_with_proof_budget(
        &projections,
        doc! {"_id":agent_id},
        session,
        snapshot,
    )
    .await
    .map_err(TransactionLogError::Driver)?
    .ok_or_else(corrupt)?;
    super::super::projection::validate_projection(projection, &agent, &head)?;
    if head.active_ordinal > 0 {
        super::super::reads::metadata::seal_at_from(
            database,
            &head,
            head.active_ordinal - 1,
            session,
            snapshot,
            None,
        )
        .await?
        .precedes(&head)?;
    }
    Ok((agent, head))
}

async fn scan(
    database: &Database,
    agent: &Agent,
    head: &Head,
    session: &mut ClientSession,
) -> Result<Binary, TransactionLogError> {
    let source = majority_collection::<RawDocumentBuf>(database, "agent_log");
    let mut cursor = source
        .find(doc! {"agent_id":agent.id.as_str(),
        "seq":{"$gte":head.active_start_seq,"$lte":head.last_seq}})
        .hint(Hint::Name("agent_log_sequence".into()))
        .sort(doc! {"seq":1})
        .limit(super::super::MAX_ENTRIES + 1)
        .batch_size(64)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut next = head.active_start_seq;
    let mut bytes = 0_i64;
    let mut leaves = Vec::new();
    while let Some(raw) = cursor
        .next(&mut *session)
        .await
        .transpose()
        .map_err(TransactionLogError::Driver)?
    {
        let length = i64::try_from(raw.as_bytes().len()).map_err(|_| corrupt())?;
        bytes = bytes
            .checked_add(length)
            .filter(|size| *size <= super::super::MAX_BYTES && length <= 16 * 1024 * 1024)
            .ok_or_else(corrupt)?;
        let entry = read_entry(&Document::try_from(&raw).map_err(|_| corrupt())?)?;
        if entry.agent_id != agent.id
            || entry.seq != next as u64
            || next > head.last_seq
            || (next == 1) != matches!(&entry.kind, AgentLogKind::Created { .. })
        {
            return Err(corrupt().into());
        }
        if let AgentLogKind::Created {
            workspace_id,
            worktree_id,
        } = &entry.kind
        {
            if workspace_id != &agent.workspace_id || worktree_id != agent.creation_worktree_id() {
                return Err(corrupt().into());
            }
        }
        leaves.extend_from_slice(
            &digest::leaf(agent.id.as_str(), &head.incarnation, next, raw.as_bytes()).bytes,
        );
        next += 1;
    }
    if next != head.last_seq + 1 || bytes != head.active_bytes {
        return Err(corrupt().into());
    }
    Ok(digest::binary(leaves))
}
