//! Read-only legacy opening projections behind existing flags/cohort proof.
use super::{agent_log::read_entry, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry, AgentUsageReading};
use mongodb::{
    bson::doc,
    options::{Hint, ReadConcern},
    ClientSession,
};

#[cfg(test)]
#[path = "agent_opening_reads/diagnostics_tests.rs"]
mod diagnostics_tests;
#[cfg(test)]
#[path = "agent_opening_reads/privacy.rs"]
mod privacy;
pub(super) mod transcript;
const INDEX: &str = "agent_log_kind_sequence";
const MAX_BYTES: usize = 4 * 1024 * 1024;

async fn eligible(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    // Cohort exclusion leaves validation to the established compatibility
    // reader; optional lookup must not add proof work for the rest of a fleet.
    if !store.bounded_log_enabled(&agent.id) {
        decision("cohort_excluded");
        return Ok(false);
    }
    let (actual, trusted) =
        super::agent_log_integrity::read::read_proven_agent(store, &agent.id, session)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
    if actual != *agent {
        return Err(SandboxStoreError::SequenceConflict);
    }
    if !trusted {
        decision("untrusted");
        return Ok(false);
    }
    let database = store.client.database(&store.agent_log.namespace().db);
    let pending = super::migrations::opening_index::inspect(&database)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.into()))?;
    if !pending.is_empty() {
        decision("index_absent");
        return Ok(false);
    }
    Ok(true)
}

pub(super) async fn usage(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentUsageReading>, SandboxStoreError> {
    if !store.bounded_usage_reads {
        return Ok(None);
    }
    if !eligible(store, agent, session).await? {
        return Ok(None);
    }
    let rows = selected(
        store,
        agent,
        session,
        Selection {
            types: &["usage_updated"],
            after: 0,
            through: agent.conversation.last_seq,
            limit: 1,
            descending: true,
        },
    )
    .await?;
    let Some(row) = rows.first() else {
        decision("summary_absent");
        return Ok(None);
    };
    let result = AgentUsageReading::from_summary(agent, row).map_err(SandboxStoreError::Corrupt)?;
    decision("usage_summary");
    Ok(Some(result))
}

struct Selection<'a> {
    types: &'a [&'a str],
    after: u64,
    through: u64,
    limit: i64,
    descending: bool,
}

async fn selected(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    selection: Selection<'_>,
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    let Selection {
        types,
        after,
        through,
        limit,
        descending,
    } = selection;
    if through > agent.conversation.last_seq || after > through || !(1..=33).contains(&limit) {
        return Err(SandboxStoreError::InvalidPage);
    }
    let after = super::sandbox_mongo::signed(after, "opening read lower bound")
        .map_err(SandboxStoreError::Corrupt)?;
    let through = super::sandbox_mongo::signed(through, "opening read upper bound")
        .map_err(SandboxStoreError::Corrupt)?;
    let mut filter =
        doc! { "agent_id": agent.id.as_str(), "seq": { "$gt": after, "$lte": through } };
    if !types.is_empty() {
        filter.insert("record.kind.type", doc! {"$in": types});
    }
    let mut read = super::mongo_read_telemetry::Read::find(store.agent_log.name());
    let collection = store
        .agent_log
        .clone_with_type::<mongodb::bson::RawDocumentBuf>();
    let mut cursor = collection
        .find(filter)
        .hint(Hint::Name(
            if types.is_empty() {
                "agent_log_sequence"
            } else {
                INDEX
            }
            .into(),
        ))
        .sort(doc! { "seq": if descending { -1 } else { 1 } })
        .limit(limit)
        .batch_size(33)
        .max_time(std::time::Duration::from_secs(2))
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    let mut result: Vec<AgentLogEntry> = Vec::new();
    let mut bytes = 0;
    while let Some(row) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(unavailable)?
    {
        bytes += row.as_bytes().len();
        if bytes > MAX_BYTES {
            return Err(SandboxStoreError::AgentLogCapacityExceeded);
        }
        let entry = read_entry(&mongodb::bson::Document::try_from(&row).map_err(|_| corrupt())?)?;
        if entry.agent_id != agent.id
            || entry.seq <= after as u64
            || entry.seq > through as u64
            || crate::store::sandbox_mongo::signed(entry.seq, "opening source sequence").is_err()
            || layer2_core::seconds_from_rfc3339(&entry.appended_at).is_none()
            || result.last().is_some_and(|old| {
                if descending {
                    old.seq <= entry.seq
                } else {
                    old.seq >= entry.seq
                }
            })
        {
            return Err(corrupt());
        }
        result.push(entry);
    }
    read.complete();
    Ok(result)
}

fn decision(outcome: &'static str) {
    use crate::diagnostic_format::{OPENING_DECISION_ROOT, OPENING_DECISION_TARGET};

    // The JSON formatter can include the current span even for a root event.
    let root = tracing::debug_span!(
        target: OPENING_DECISION_TARGET,
        parent: None,
        OPENING_DECISION_ROOT
    );
    if root.is_disabled() {
        return;
    }
    let _entered = root.enter();
    tracing::debug!(
        target: OPENING_DECISION_TARGET, parent: &root, outcome,
        "Agent opening legacy projection decision"
    );
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("Agent opening evidence does not match its captured source".into())
}
