//! Read-only service snapshot for runtime retirement preview. Never opens stores
//! through startup initialization or writes Mongo lifecycle state.
use crate::store::transaction_terminal::GuardedTerminal as _;
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::Path, time::Duration};

use layer2_core::sandbox::{store::SandboxStoreError, Agent, SandboxHierarchyRecord};
use layer3_adapter::runtime_ledger_retirement::{
    RuntimeLedgerRetirementSnapshot, MAX_LEDGER_RETIREMENT_RESOURCES,
    MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES,
};
use mongodb::{
    bson::{doc, Document},
    Client, ClientSession, Database,
};
use sha2::{Digest, Sha256};

use super::{
    agents::read_agent, sandboxes::read_sandbox, workspaces::read_workspace,
    worktrees::read_worktree,
};

type Result<T> = std::result::Result<T, &'static str>;
const INVALID: &str = "runtime ledger retirement observation is unavailable or inconsistent";
const OUTPUT: &str = "runtime ledger retirement observation output could not be published";
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;

pub(crate) async fn command(sandbox: &str, destination: &Path) -> Result<()> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err(OUTPUT);
    }
    let uri = std::env::var("CHRONO_MONGODB_URI").map_err(|_| INVALID)?;
    let database = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| INVALID)?;
    let mode = std::env::var("CHRONO_MONGODB_AUTHORITY_MODE").ok();
    let authority = cma_service_auth::mongodb_authority::MongoAuthorityMode::parse(mode.as_deref())
        .map_err(|_| INVALID)?;
    let snapshot = tokio::time::timeout(Duration::from_secs(60), async {
        let mut options = mongodb::options::ClientOptions::parse(uri)
            .await
            .map_err(|_| INVALID)?;
        super::transaction_barrier::bind(&mut options, &database);
        let client = Client::with_options(options).map_err(|_| INVALID)?;
        let database = client.database(&database);
        super::isolation::verify_with_authority(&database, authority)
            .await
            .map_err(|_| INVALID)?;
        observe(&client, &database, sandbox).await
    })
    .await
    .map_err(|_| INVALID)??;
    let bytes = serde_json::to_vec(&snapshot).map_err(|_| OUTPUT)?;
    if bytes.len() > MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES {
        return Err(OUTPUT);
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    publish(destination, &bytes)?;
    println!(
        "{}",
        serde_json::json!({
            "format": "runtime-ledger-retirement-observation-summary-v1",
            "snapshot_sha256": digest, "source_sha256": snapshot.source_sha256,
            "agents_observed": snapshot.agents_observed,
            "worktrees_observed": snapshot.worktrees_observed,
            "settled_deleted_agents": snapshot.deleted_agents.len(),
            "settled_removed_worktrees": snapshot.removed_worktrees.len(),
            "apply_authority": false, "quiescence_verified": false,
        })
    );
    Ok(())
}

pub(super) async fn observe(
    client: &Client,
    database: &Database,
    sandbox: &str,
) -> Result<RuntimeLedgerRetirementSnapshot> {
    if sandbox.is_empty() || sandbox.len() > 256 || sandbox.chars().any(char::is_control) {
        return Err(INVALID);
    }
    let mut session = client.start_session().await.map_err(|_| INVALID)?;
    super::sandbox_mongo::start_transaction(&mut session)
        .await
        .map_err(|_| INVALID)?;
    let result = read(database, sandbox, &mut session).await;
    // A read-only transaction is always aborted, including successful output.
    let aborted = session.abort_guarded().await.map_err(|_| INVALID);
    aborted?;
    result
}

async fn read(
    database: &Database,
    sandbox: &str,
    session: &mut ClientSession,
) -> Result<RuntimeLedgerRetirementSnapshot> {
    let mut evidence = Evidence {
        digest: Sha256::new(),
        bytes: 0,
    };
    let document = database
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":sandbox})
        .max_time(Duration::from_secs(10))
        .session(&mut *session)
        .await
        .map_err(|_| INVALID)?
        .ok_or(INVALID)?;
    evidence.record("sandboxes", &document)?;
    let parent = read_sandbox(&document).map_err(|_| INVALID)?;
    if parent.id.as_str() != sandbox {
        return Err(INVALID);
    }
    let workspaces = children(
        database,
        "workspaces",
        scope_filter(sandbox, None),
        session,
        &mut evidence,
        read_workspace,
    )
    .await?;
    let workspace_ids = workspaces
        .iter()
        .map(|value| value.id.to_string())
        .collect();
    let worktrees = children(
        database,
        "worktrees",
        scope_filter(sandbox, Some(("workspace_id", workspace_ids))),
        session,
        &mut evidence,
        read_worktree,
    )
    .await?;
    let worktree_ids = worktrees.iter().map(|value| value.id.to_string()).collect();
    let agents = children(
        database,
        "agents",
        scope_filter(sandbox, Some(("worktree_id", worktree_ids))),
        session,
        &mut evidence,
        read_retirement_agent,
    )
    .await?;
    let snapshot = SandboxHierarchyRecord {
        sandbox: parent,
        workspaces,
        worktrees,
        agents,
    };
    snapshot
        .ledger_retirement_observation(
            layer2_core::now_rfc3339(),
            format!("{:x}", evidence.digest.finalize()),
        )
        .map_err(|_| INVALID)
}

fn read_retirement_agent(document: &Document) -> std::result::Result<Agent, SandboxStoreError> {
    // A terminal aggregate cannot override an unresolved integrity finding.
    // Presence also holds for malformed or future marker formats; observing
    // retirement must never clear or reinterpret that evidence as eligibility.
    if document.contains_key(super::agent_log_integrity::QUARANTINE_FIELD) {
        return Err(SandboxStoreError::Corrupt(
            "Agent log integrity is quarantined".into(),
        ));
    }
    read_agent(document)
}

struct Evidence {
    digest: Sha256,
    bytes: usize,
}

impl Evidence {
    fn record(&mut self, collection: &str, document: &Document) -> Result<()> {
        let bytes = mongodb::bson::serialize_to_vec(document).map_err(|_| INVALID)?;
        self.bytes = self.bytes.checked_add(bytes.len()).ok_or(INVALID)?;
        if self.bytes > MAX_SOURCE_BYTES {
            return Err(INVALID);
        }
        self.digest.update((collection.len() as u64).to_be_bytes());
        self.digest.update(collection.as_bytes());
        self.digest.update((bytes.len() as u64).to_be_bytes());
        self.digest.update(bytes);
        Ok(())
    }
}

// Read through both authoritative/projection Sandbox lineage and retained
// parent references. A child with corrupt Sandbox fields must not hide while
// still pointing at a Worktree whose records might otherwise be selected.
fn scope_filter(sandbox: &str, parents: Option<(&str, Vec<String>)>) -> Document {
    let mut terms = vec![
        doc! {"sandbox_id":sandbox},
        doc! {"record.sandbox_id":sandbox},
    ];
    if let Some((field, ids)) = parents {
        for key in [field.to_owned(), format!("record.{field}")] {
            let mut term = Document::new();
            term.insert(key, doc! {"$in": &ids});
            terms.push(term);
        }
    }
    doc! {"$or":terms}
}

async fn children<T>(
    database: &Database,
    collection: &str,
    filter: Document,
    session: &mut ClientSession,
    evidence: &mut Evidence,
    parse: fn(&Document) -> std::result::Result<T, SandboxStoreError>,
) -> Result<Vec<T>> {
    let mut cursor = database
        .collection::<Document>(collection)
        .find(filter)
        .sort(doc! {"_id":1})
        .limit((MAX_LEDGER_RETIREMENT_RESOURCES + 1) as i64)
        .max_time(Duration::from_secs(10))
        .session(&mut *session)
        .await
        .map_err(|_| INVALID)?;
    let mut values = Vec::new();
    while cursor.advance(&mut *session).await.map_err(|_| INVALID)? {
        if values.len() == MAX_LEDGER_RETIREMENT_RESOURCES {
            return Err(INVALID);
        }
        let document = cursor.deserialize_current().map_err(|_| INVALID)?;
        evidence.record(collection, &document)?;
        values.push(parse(&document).map_err(|_| INVALID)?);
    }
    Ok(values)
}

fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or(OUTPUT)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| OUTPUT)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::File::open(parent)?.sync_all());
    if result.is_err() {
        // Leave an invalid owned file instead of unlinking a possibly replaced
        // name. Only a completely published artifact is reported to the caller.
        let _ = file.set_len(0);
        let _ = file.sync_all();
        return Err(OUTPUT);
    }
    Ok(())
}

#[cfg(test)]
#[path = "ledger_retirement_tests.rs"]
mod tests;
