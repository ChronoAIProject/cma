//! Bounded complete hierarchy and lane reads inside the caller's transaction.

use std::time::Duration;

use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal as Refusal;
use layer2_core::sandbox::ledger_maintenance_plan::{
    LedgerMaintenanceLaneV1, LedgerMaintenanceSnapshotV1, MAX_LEDGER_MAINTENANCE_RESOURCES,
};
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceInspection, LedgerMaintenanceStoreError as Domain,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::{Caller, Capability, SandboxId};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use sha2::{Digest, Sha256};

use super::ledger_maintenance::{refused, ApplyError};
use super::ledger_maintenance_pre_exclusion_records as preparation;
use super::ledger_maintenance_records;
use super::sandbox_mongo::MongoSandboxStore;

pub(super) struct Snapshot {
    pub inspection: LedgerMaintenanceInspection,
    pub pre_exclusion:
        Option<layer2_core::sandbox::ledger_maintenance_pre_exclusion::LedgerPreExclusionStateV1>,
    pub parent: Document,
    /// Actual rows, including sidecars, are retained for same-transaction fences.
    pub children: Vec<(Collection<Document>, Document)>,
}

pub(super) async fn load(
    store: &MongoSandboxStore,
    caller: &Caller,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<Snapshot, ApplyError> {
    load_version(store, caller, sandbox, session, false).await
}

pub(super) async fn load_pre_exclusion(
    store: &MongoSandboxStore,
    caller: &Caller,
    sandbox: &SandboxId,
    session: &mut ClientSession,
) -> Result<Snapshot, ApplyError> {
    load_version(store, caller, sandbox, session, true).await
}

async fn load_version(
    store: &MongoSandboxStore,
    caller: &Caller,
    sandbox: &SandboxId,
    session: &mut ClientSession,
    pre_exclusion_reader: bool,
) -> Result<Snapshot, ApplyError> {
    let parent = store
        .sandboxes
        .find_one(doc! {"_id": sandbox.as_str()})
        .session(&mut *session)
        .await?
        .ok_or_else(|| refused(Refusal::NotFound))?;
    // Conceal foreign records before parsing their private maintenance marker.
    if !caller.authorization.allows(Capability::ControlAnySandbox)
        && (!caller.authorization.allows(Capability::UseOwnSandboxes)
            || parent.get_str("owner").ok() != Some(caller.subject.as_str()))
    {
        return Err(refused(Refusal::NotFound));
    }
    let pre_exclusion = preparation::read(&parent)?;
    if pre_exclusion.is_some() && !pre_exclusion_reader {
        return Err(refused(Refusal::Conflict));
    }
    let source_parent = preparation::source_document(&parent)?;
    let sandbox = super::sandboxes::read_sandbox(&source_parent).map_err(|_| Domain::Corrupt)?;
    if !caller.authorization.allows(Capability::ControlAnySandbox)
        && caller.subject != sandbox.owner
    {
        return Err(refused(Refusal::NotFound));
    }
    let state = ledger_maintenance_records::read(&parent)?;
    if state
        .as_ref()
        .is_some_and(|state| state.record.binding.sandbox_id != sandbox.id.as_str())
    {
        return Err(Domain::Corrupt.into());
    }
    if let Some(state) = &state {
        let hash = ledger_maintenance_records::state_hash(state)?;
        let receipt = super::ledger_maintenance_transaction::audit_collection(store)
            .find_one(doc! {"_id":&hash})
            .session(&mut *session)
            .await?
            .ok_or(Domain::Corrupt)?;
        ledger_maintenance_records::validate_audit(&receipt, state)?;
    }
    if let Some(state) = &pre_exclusion {
        let accepted = state.accepted().map_err(|_| Domain::Corrupt)?;
        let receipt = super::ledger_maintenance_pre_exclusion_transaction::audit_collection(store)
            .find_one(doc! {"_id":preparation::audit_key(&accepted, state.record_revision)})
            .session(&mut *session)
            .await?
            .ok_or(Domain::Corrupt)?;
        if preparation::outcome(&receipt, &accepted)? != *state {
            return Err(Domain::Corrupt.into());
        }
    }
    let mut budget = ReadBudget {
        rows: 0,
        bytes: 0,
        digest: if pre_exclusion_reader {
            Sha256::new_with_prefix(b"chrono.runtime.ledger.pre-exclusion-source.v1\0")
        } else {
            Sha256::new_with_prefix(b"chrono.runtime.ledger.maintenance-source.v1\0")
        },
        pre_exclusion_reader,
        children: Vec::new(),
    };
    budget.count("sandboxes", &parent)?;
    let workspaces = children(
        &store.workspaces,
        scope(sandbox.id.as_str(), None),
        session,
        &mut budget,
        super::workspaces::read_workspace,
    )
    .await?;
    let worktrees = children(
        &store.worktrees,
        scope(
            sandbox.id.as_str(),
            Some((
                "workspace_id",
                workspaces.iter().map(|row| row.id.to_string()).collect(),
            )),
        ),
        session,
        &mut budget,
        super::worktrees::read_worktree,
    )
    .await?;
    let agents = children(
        &store.agents,
        scope(
            sandbox.id.as_str(),
            Some((
                "worktree_id",
                worktrees.iter().map(|row| row.id.to_string()).collect(),
            )),
        ),
        session,
        &mut budget,
        super::agents::read_agent,
    )
    .await?;
    // Inspection must not return a foreign or incomplete child hierarchy merely
    // because one corrupt projected relationship matched the parent query.
    layer2_core::sandbox::SandboxHierarchyRecord {
        sandbox: sandbox.clone(),
        workspaces: workspaces.clone(),
        worktrees: worktrees.clone(),
        agents: agents.clone(),
    }
    .ledger_retirement_observation(layer2_core::now_rfc3339(), "0".repeat(64))
    .map_err(|_| Domain::Corrupt)?;
    let mut command_lanes = Vec::with_capacity(agents.len());
    for agent in &agents {
        let row = store
            .agent_command_orders
            .find_one(doc! {"_id":agent.id.as_str()})
            .max_time(Duration::from_secs(5))
            .session(&mut *session)
            .await?;
        let lane = match row {
            Some(document) => {
                budget.count("agent_command_orders", &document)?;
                let lane = super::agent_commands::read_command_lane(&document, agent.id.as_str())
                    .map_err(|_| Domain::Corrupt)?;
                budget
                    .children
                    .push((store.agent_command_orders.clone(), document));
                Some(lane)
            }
            None => None,
        };
        command_lanes.push(LedgerMaintenanceLaneV1 {
            agent_id: agent.id.clone(),
            lane,
        });
    }
    let sandbox_event_head =
        u64::try_from(parent.get_i64("last_seq").map_err(|_| Domain::Corrupt)?)
            .map_err(|_| Domain::Corrupt)?;
    Ok(Snapshot {
        inspection: LedgerMaintenanceInspection {
            source: LedgerMaintenanceSnapshotV1 {
                source_documents_sha256: format!("{:x}", budget.digest.finalize()),
                sandbox,
                sandbox_event_head,
                agents,
                workspaces,
                worktrees,
                command_lanes,
            },
            state,
        },
        parent,
        pre_exclusion,
        children: budget.children,
    })
}

// Both projection and authoritative relationships participate. A corrupt
// Sandbox projection cannot hide a child still referencing this hierarchy.
fn scope(sandbox: &str, parents: Option<(&str, Vec<String>)>) -> Document {
    let mut terms = vec![
        doc! {"sandbox_id":sandbox},
        doc! {"record.sandbox_id":sandbox},
    ];
    if let Some((field, ids)) = parents {
        for field in [field.to_owned(), format!("record.{field}")] {
            let mut term = Document::new();
            term.insert(field, doc! {"$in": &ids});
            terms.push(term);
        }
    }
    doc! {"$or": terms}
}

struct ReadBudget {
    rows: usize,
    bytes: usize,
    digest: Sha256,
    pre_exclusion_reader: bool,
    children: Vec<(Collection<Document>, Document)>,
}

impl ReadBudget {
    fn count(&mut self, collection: &str, row: &Document) -> Result<(), Domain> {
        self.bytes = self
            .bytes
            .checked_add(
                mongodb::bson::serialize_to_vec(row)
                    .map_err(|_| Domain::Corrupt)?
                    .len(),
            )
            .ok_or(Domain::Corrupt)?;
        if self.bytes > 32 * 1024 * 1024 {
            return Err(Domain::Corrupt);
        }
        let mut source = row.clone();
        source.remove("ledger_maintenance_fence_v1");
        if collection == "sandboxes" {
            source = preparation::source_document(&source)?;
            if self.pre_exclusion_reader {
                // The new E1 hash grammar normalizes only its own version
                // envelope. Existing V1 snapshot digests are unchanged.
                source.insert(
                    "document_schema_version",
                    super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
                );
            }
            source.remove(ledger_maintenance_records::FIELD);
        }
        let bytes = mongodb::bson::serialize_to_vec(&source).map_err(|_| Domain::Corrupt)?;
        self.digest.update((collection.len() as u64).to_be_bytes());
        self.digest.update(collection.as_bytes());
        self.digest.update((bytes.len() as u64).to_be_bytes());
        self.digest.update(bytes);
        Ok(())
    }
}

async fn children<T>(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
    budget: &mut ReadBudget,
    parse: fn(&Document) -> Result<T, SandboxStoreError>,
) -> Result<Vec<T>, ApplyError> {
    let mut cursor = collection
        .find(filter)
        .sort(doc! {"_id":1})
        .limit((MAX_LEDGER_MAINTENANCE_RESOURCES + 1) as i64)
        .max_time(Duration::from_secs(5))
        .session(&mut *session)
        .await?;
    let mut rows = Vec::new();
    while cursor.advance(&mut *session).await? {
        budget.rows += 1;
        if budget.rows > MAX_LEDGER_MAINTENANCE_RESOURCES {
            return Err(Domain::Corrupt.into());
        }
        let row = cursor.deserialize_current()?;
        budget.count(&collection.namespace().coll, &row)?;
        rows.push(parse(&row).map_err(|_| Domain::Corrupt)?);
        budget.children.push((collection.clone(), row));
    }
    Ok(rows)
}
