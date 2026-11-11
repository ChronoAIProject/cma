//! Hold, source fences and immutable receipt share one Mongo transaction.

use layer2_core::sandbox::ledger_maintenance_admission::{
    plan_ledger_maintenance_admission, plan_ledger_maintenance_takeover,
    LedgerMaintenanceAdmissionRefusal as Refusal, LedgerMaintenanceAdmissionRequest,
    LedgerMaintenanceAdmissionV1, LedgerMaintenanceStateV1, LedgerMaintenanceTakeoverRequest,
};
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as Domain;
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};

use super::ledger_maintenance::{refused, ApplyError, Change};
use super::ledger_maintenance_records::{self as records, AUDITS, FIELD};
use super::ledger_maintenance_snapshot;
use super::sandbox_mongo::MongoSandboxStore;

pub(super) struct Receipt {
    pub state: LedgerMaintenanceStateV1,
    pub audit: Option<Document>,
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    change: &Change,
    session: &mut ClientSession,
) -> Result<Receipt, ApplyError> {
    if !change.policy().enabled {
        return Err(refused(Refusal::Disabled));
    }
    let snapshot =
        ledger_maintenance_snapshot::load(store, change.caller(), &change.sandbox(), session)
            .await?;
    let existing = snapshot.inspection.state.as_ref();
    let source = &snapshot.inspection.source;
    let next = match change {
        Change::Admit(policy, request) => {
            match plan_ledger_maintenance_admission(
                *policy,
                source,
                existing,
                LedgerMaintenanceAdmissionRequest {
                    caller: &request.caller,
                    accepted: &request.accepted,
                    lease: request.lease.clone(),
                    now: &request.now,
                },
            )
            .map_err(Domain::from)?
            {
                LedgerMaintenanceAdmissionV1::Replay(state) => {
                    return Ok(Receipt {
                        state: *state,
                        audit: None,
                    });
                }
                LedgerMaintenanceAdmissionV1::Persist(state) => *state,
            }
        }
        Change::Takeover(_, request) => {
            // Derive the exact original outcome for uncertain-commit replay.
            // The durable audit remains decisive even after a later takeover.
            let desired = plan_ledger_maintenance_takeover(
                &request.expected,
                &request.expected_source,
                LedgerMaintenanceTakeoverRequest {
                    caller: &request.caller,
                    expected: &request.expected,
                    expected_source: &request.expected_source,
                    lease: request.lease.clone(),
                    now: &request.now,
                },
            )
            .map_err(Domain::from)?;
            let audit = records::audit(Some(&request.expected), &desired, &request.caller)?;
            if audit_present(store, &audit, session).await? {
                return Ok(Receipt {
                    state: desired,
                    audit: None,
                });
            }
            plan_ledger_maintenance_takeover(
                existing.ok_or_else(|| refused(Refusal::Conflict))?,
                source,
                LedgerMaintenanceTakeoverRequest {
                    caller: &request.caller,
                    expected: &request.expected,
                    expected_source: &request.expected_source,
                    lease: request.lease.clone(),
                    now: &request.now,
                },
            )
            .map_err(Domain::from)?
        }
    };
    for field in [
        "channel_reuse_cleanup_execution",
        "channel_retirement_finalization",
        "owned_workspace_cleanup_admission_v1",
    ] {
        if snapshot.parent.contains_key(field)
            || snapshot
                .children
                .iter()
                .any(|(_, row)| row.contains_key(field))
        {
            return Err(refused(Refusal::Conflict));
        }
    }
    let encoded = records::encode(&next)?;
    // Change each existing row, including a command lane with no active lease.
    // Mongo snapshot write conflicts serialize any concurrent change to them.
    // Absent lanes stay absent: A's atomic parent guard fences their creation.
    for (collection, document) in &snapshot.children {
        let result = collection
            .update_one(
                document.clone(),
                doc! {"$inc":{"ledger_maintenance_fence_v1":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(refused(Refusal::Conflict));
        }
    }
    let mut fields = Document::new();
    fields.insert(FIELD, encoded);
    let result = store
        .sandboxes
        .update_one(
            snapshot.parent,
            doc! {"$set":fields, "$inc":{"ledger_maintenance_fence_v1":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if result.matched_count != 1 {
        return Err(refused(Refusal::Conflict));
    }
    let audit = records::audit(existing, &next, change.caller())?;
    audit_collection(store)
        .insert_one(audit.clone())
        .session(session)
        .await?;
    Ok(Receipt {
        state: next,
        audit: Some(audit),
    })
}

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    audit: &Document,
    time: Timestamp,
) -> Result<bool, Domain> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(|_| Domain::PendingCommit)?;
    let actual = audit_collection(store)
        .find_one(doc! {"_id":audit.get("_id").ok_or(Domain::Corrupt)?})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|_| Domain::PendingCommit)?;
    Ok(actual.as_ref() == Some(audit))
}

async fn audit_present(
    store: &MongoSandboxStore,
    expected: &Document,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let actual = audit_collection(store)
        .find_one(doc! {"_id":expected.get("_id").ok_or(Domain::Corrupt)?})
        .session(session)
        .await?;
    match actual {
        None => Ok(false),
        Some(actual) if &actual == expected => Ok(true),
        Some(_) => Err(Domain::Corrupt.into()),
    }
}

pub(super) fn audit_collection(store: &MongoSandboxStore) -> mongodb::Collection<Document> {
    store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection(AUDITS)
}
