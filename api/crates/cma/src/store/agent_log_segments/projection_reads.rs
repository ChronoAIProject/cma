use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogAnswerEvidence, AgentLogAnswerLookup, AgentLogKind,
    AgentLogProjectionState,
};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use super::{
    corrupt,
    projection::{self, EvidenceRecord},
    read_format, read_head, Head,
};
use crate::store::{
    agent_log_integrity::TransactionLogError, agents::read_agent, sandbox_mongo::MongoSandboxStore,
};

fn error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}

pub(super) async fn snapshot(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Option<(Head, ClientSession)>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?;
    let aggregate = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_format(&aggregate)?.is_none() {
        return Ok(None);
    }
    if read_agent(&aggregate)? != *agent {
        return Err(SandboxStoreError::Conflict);
    }
    let head = read_head(store, &aggregate, &mut session, true)
        .await
        .map_err(error)?;
    Ok(Some((head, session)))
}

pub(in crate::store) async fn read_projection(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Option<AgentLogProjectionState>, SandboxStoreError> {
    let Some((head, mut session)) = snapshot(store, agent).await? else {
        return Ok(None);
    };
    projection::read(store, agent, &head, &mut session)
        .await
        .map(Some)
        .map_err(error)
}

pub(in crate::store) async fn final_answer(
    store: &MongoSandboxStore,
    agent: &Agent,
    sequence: u64,
) -> Result<AgentLogAnswerLookup, SandboxStoreError> {
    let Some((head, mut session)) = snapshot(store, agent).await? else {
        return Ok(AgentLogAnswerLookup::Legacy);
    };
    projection::read(store, agent, &head, &mut session)
        .await
        .map_err(error)?;
    if sequence == 0 || sequence > agent.conversation.last_seq {
        return Ok(AgentLogAnswerLookup::NotFinal);
    }
    let message =
        projection::evidence(store, &head, "message", &sequence.to_string(), &mut session)
            .await
            .map_err(error)?;
    let Some(EvidenceRecord::Message {
        sequence: actual,
        turn_sequence,
        role,
    }) = message
    else {
        let source = super::consumers::points(store, &head, &[sequence], &mut session, true)
            .await
            .map_err(error)?;
        if matches!(source[0].kind, AgentLogKind::AgentMessage { .. }) {
            return Err(corrupt());
        }
        return Ok(AgentLogAnswerLookup::NotFinal);
    };
    if actual != sequence || turn_sequence == 0 || turn_sequence > head.last_seq as u64 {
        return Err(corrupt());
    }
    let turn = projection::evidence(
        store,
        &head,
        "turn",
        &turn_sequence.to_string(),
        &mut session,
    )
    .await
    .map_err(error)?;
    let Some(EvidenceRecord::Turn { facts }) = turn else {
        return Err(corrupt());
    };
    if facts.turn_sequence != turn_sequence
        || facts
            .end_sequence
            .is_some_and(|end| end > head.last_seq as u64)
    {
        return Err(corrupt());
    }
    if facts.final_answer_sequence() != Some(sequence) {
        return Ok(AgentLogAnswerLookup::NotFinal);
    }
    if role != layer2_core::sandbox::AgentTranscriptRole::Agent {
        return Err(corrupt());
    }
    let end = facts.end_sequence.ok_or_else(corrupt)?;
    let entries = super::consumers::points(
        store,
        &head,
        &[turn_sequence, sequence, end],
        &mut session,
        true,
    )
    .await
    .map_err(error)?;
    if entries.len() != 3 {
        return Err(corrupt());
    }
    let mut entries = entries.into_iter();
    Ok(AgentLogAnswerLookup::Answer(Box::new(
        AgentLogAnswerEvidence {
            facts,
            start: entries.next().ok_or_else(corrupt)?,
            message: entries.next().ok_or_else(corrupt)?,
            terminal: entries.next().ok_or_else(corrupt)?,
        },
    )))
}
