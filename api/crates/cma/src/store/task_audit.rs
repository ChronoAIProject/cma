//! Exact selected message receipts plus read-only terminal evidence checks.
//! Neither sampling nor the explicit query can dispatch, settle or repair work.
use super::sandbox_mongo::MongoSandboxStore;
use crate::config::task_audit::Selection;
use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::{sandbox::agent_io::AdmittedAgentCommand, AgentId};
use mongodb::{
    bson::{doc, Binary},
    options::{Hint, ReadConcern},
    ClientSession, Database,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod cli;
mod evidence;
mod report;
#[cfg(test)]
mod tests;
pub(crate) use cli::command;
use report::{Entry, Report};
const INDEX: &str = "agent_command_admission_cohort";
const MAX_ROWS: u64 = 10_000;
const MAX_BYTES: usize = 16 * 1024 * 1024;
type Result<T> = std::result::Result<T, &'static str>;

pub(in crate::store) async fn observe(
    store: &MongoSandboxStore,
    selection: &Selection,
) -> Result<Report> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let observed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "invalid_selection")?
            .as_secs();
        let database = store.client.database(&store.agent_commands.namespace().db);
        let identity = super::migrations::require_command_cohort_index(&database)
            .await
            .map_err(|_| "database")?;
        observe_snapshot(store, selection, observed, &database, &identity).await
    })
    .await
    .map_err(|_| "timeout")?
}

async fn observe_snapshot(
    store: &MongoSandboxStore,
    selection: &Selection,
    observed: u64,
    database: &Database,
    identity: &Binary,
) -> Result<Report> {
    let (from, to) = selection.bounds(observed)?;
    let mut session = store.client.start_session().await.map_err(|_| "database")?;
    // Deliberately bypass ordinary work transaction setup: it can publish a
    // work-authority fence. This transaction reads and aborts, never commits.
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .await
        .map_err(|_| "database")?;
    let result = async {
        let report = read(store, selection, from, to, observed, &mut session).await?;
        // Recheck while the snapshot is open. A drop before its first read can
        // otherwise make the hinted count and find return a false healthy zero.
        let current = super::migrations::require_command_cohort_index(database)
            .await
            .map_err(|_| "database")?;
        if &current != identity {
            return Err("invalid_snapshot");
        }
        Ok(report)
    }
    .await;
    session.abort_guarded().await.map_err(|_| "database")?;
    result
}

async fn read(
    store: &MongoSandboxStore,
    selection: &Selection,
    from: u64,
    to: u64,
    observed: u64,
    session: &mut ClientSession,
) -> Result<Report> {
    let stamp = |at| layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(at));
    let filter = doc! {"record.receipt.admitted_at":{"$gte":stamp(from),"$lt":stamp(to)},
    "record.receipt.envelope.command.type":"message"};
    let accepted = store
        .agent_commands
        .count_documents(filter.clone())
        .hint(Hint::Name(INDEX.into()))
        .limit(MAX_ROWS + 1)
        .max_time(Duration::from_secs(1))
        .session(&mut *session)
        .await
        .map_err(|_| "database")?;
    if accepted > MAX_ROWS {
        return Err("budget");
    }
    let mut cursor = store
        .agent_commands
        .find(filter)
        .sort(doc! {"record.receipt.admitted_at":1,"_id":1})
        .hint(Hint::Name(INDEX.into()))
        .limit((MAX_ROWS + 1) as i64)
        .batch_size(50)
        .max_time(Duration::from_secs(1))
        .session(&mut *session)
        .await
        .map_err(|_| "database")?;
    let mut entries = Vec::new();
    let mut dispatched: BTreeMap<AgentId, Vec<(usize, AdmittedAgentCommand)>> = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut used = 0;
    let mut count = 0;
    loop {
        let row = {
            cursor
                .stream(&mut *session)
                .try_next()
                .await
                .map_err(|_| "database")?
        };
        let Some(row) = row else {
            break;
        };
        count += 1;
        used += mongodb::bson::serialize_to_vec(&row)
            .map_err(|_| "invalid_snapshot")?
            .len();
        if count > MAX_ROWS || used > MAX_BYTES {
            return Err("budget");
        }
        let id = row.get_str("_id").map_err(|_| "invalid_snapshot")?;
        if id.is_empty() || !ids.insert(id.to_owned()) {
            return Err("invalid_snapshot");
        }
        let mut entry = Entry::invalid(id);
        if let Ok(command) = super::agent_commands::records::read(&row) {
            let at = layer2_core::seconds_from_rfc3339(&command.receipt.admitted_at)
                .ok_or("invalid_snapshot")?;
            if at < from || at >= to {
                return Err("invalid_snapshot");
            }
            entry = Entry::from_command(&command, observed);
            let identity = (command.scope(), command.key_sha256.clone());
            if !keys.insert(identity) {
                entry.result = "duplicate_admission";
            } else if command.work.dispatch.is_none() {
                entry.result = evidence::undispatched(&command);
            } else {
                dispatched
                    .entry(command.receipt.envelope.agent_id.clone())
                    .or_default()
                    .push((entries.len(), command));
            }
        }
        entries.push(entry);
    }
    if count != accepted {
        return Err("invalid_snapshot");
    }
    // Keep admission order in the private report while sharing each Agent's
    // evidence reads inside the same read-only snapshot.
    for (agent_id, commands) in dispatched {
        evidence::classify_agent(store, &agent_id, &commands, &mut entries, session).await?;
    }
    let mut report = Report::new(selection.clone(), from, to, observed, accepted);
    for mut entry in entries {
        entry.overdue = !entry.has_terminal()
            && entry
                .age_seconds
                .is_some_and(|age| age >= selection.overdue_seconds);
        report.push(entry);
    }
    report.operation_time_watermark = session.operation_time().map(|at| (at.time, at.increment));
    Ok(report)
}
