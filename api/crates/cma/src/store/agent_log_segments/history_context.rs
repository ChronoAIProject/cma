//! Exact turn context for a page beginning in the middle of a long turn.
use super::{consumers::points, corrupt, projection, projection_reads::snapshot, Head};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogKind, AgentLogTurnFacts, AgentLogTurnLookup,
};
use mongodb::{bson::doc, ClientSession};

fn error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}

pub(in crate::store) async fn turn_at(
    store: &MongoSandboxStore,
    agent: &Agent,
    through: u64,
) -> Result<AgentLogTurnLookup, SandboxStoreError> {
    if through > agent.conversation.last_seq {
        return Err(SandboxStoreError::InvalidPage);
    }
    let Some((head, mut session)) = snapshot(store, agent).await? else {
        return Ok(AgentLogTurnLookup::Legacy);
    };
    let state = projection::read(store, agent, &head, &mut session)
        .await
        .map_err(error)?;
    let previous = neighbor(store, &head, through, true, &mut session).await?;
    let next = neighbor(store, &head, through, false, &mut session).await?;
    let sequence = previous.as_ref().map(|facts| facts.turn_sequence);
    if next
        .as_ref()
        .map_or(state.latest_turn != sequence, |facts| {
            facts.previous_turn != sequence
        })
    {
        return Err(corrupt());
    }
    let Some(facts) = previous else {
        return Ok(AgentLogTurnLookup::Inactive);
    };
    if facts.end_sequence.is_some_and(|end| end <= through) {
        return Ok(AgentLogTurnLookup::Inactive);
    }
    let gap_sequence = facts.first_gap_sequence.filter(|seq| *seq <= through);
    let references: Vec<_> = std::iter::once(facts.turn_sequence)
        .chain(gap_sequence)
        .collect();
    let mut entries = points(store, &head, &references, &mut session, true)
        .await
        .map_err(error)?
        .into_iter();
    let start = entries.next().ok_or_else(corrupt)?;
    if start.appended_at != facts.started_at
        || !matches!(&start.kind, AgentLogKind::TurnStarted { turn_sequence, origin_key, .. }
            if *turn_sequence == facts.turn_sequence && origin_key == &facts.origin_key)
    {
        return Err(corrupt());
    }
    let gap = entries.next().map(Box::new);
    if gap.as_ref().map(|entry| entry.seq) != gap_sequence
        || gap.as_ref().is_some_and(
            |entry| !matches!(entry.kind, AgentLogKind::ConversationGap { frames } if frames > 0),
        )
    {
        return Err(corrupt());
    }

    Ok(AgentLogTurnLookup::Active {
        start: Box::new(start),
        gap,
    })
}

async fn neighbor(
    store: &MongoSandboxStore,
    head: &Head,
    through: u64,
    previous: bool,
    session: &mut ClientSession,
) -> Result<Option<AgentLogTurnFacts>, SandboxStoreError> {
    let mut bound = doc! {};
    bound.insert(
        if previous { "$lte" } else { "$gt" },
        i64::try_from(through).map_err(|_| corrupt())?,
    );
    let row = store
        .segment_logs
        .evidence
        .find_one(doc! {
            "agent_id":&head.agent_id,"incarnation":&head.incarnation,
            "evidence_generation":&head.evidence_generation,"kind":"turn",
            "record.facts.turn_sequence":bound,
        })
        .sort(doc! {"record.facts.turn_sequence":if previous { -1 } else { 1 }})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let key = row.get_str("entity_key").map_err(|_| corrupt())?.to_owned();
    let projection::EvidenceRecord::Turn { facts } =
        projection::read_evidence(head, "turn", &key, row)?
    else {
        return Err(corrupt());
    };
    if (facts.turn_sequence <= through) != previous {
        return Err(corrupt());
    }
    Ok(Some(facts))
}
