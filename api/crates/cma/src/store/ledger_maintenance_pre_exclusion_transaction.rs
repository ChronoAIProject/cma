//! E1 marker, complete source fences and original audit share one transaction.

use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal as Refusal;
use layer2_core::sandbox::ledger_maintenance_pre_exclusion::LedgerPreExclusionStateV1 as State;
use layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::{
    plan_pre_exclusion_admission, plan_pre_exclusion_progress,
    LedgerPreExclusionAdmissionDecision as Decision, LedgerPreExclusionPolicy as Policy,
};
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as Domain;
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};

use super::ledger_maintenance::{refused, ApplyError};
use super::ledger_maintenance_pre_exclusion::Change;
use super::ledger_maintenance_pre_exclusion_records as records;
use super::ledger_maintenance_snapshot;
use super::sandbox_mongo::MongoSandboxStore;

pub(super) struct Receipt {
    pub state: State,
    pub audit: Option<Document>,
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    policy: &Policy,
    change: &Change,
    session: &mut ClientSession,
) -> Result<Receipt, ApplyError> {
    let snapshot = ledger_maintenance_snapshot::load_pre_exclusion(
        store,
        change.caller(),
        change.sandbox(),
        session,
    )
    .await?;
    // An existing V1 operation can never be reinterpreted as pre-exclusion E1.
    if snapshot.inspection.state.is_some() {
        return Err(refused(Refusal::Conflict));
    }
    let existing = snapshot.pre_exclusion.as_ref();
    let source = &snapshot.inspection.source;
    let next = match change {
        Change::Admit(request) => {
            match plan_pre_exclusion_admission(policy, source, existing, request)
                .map_err(Domain::from)?
            {
                Decision::Persist(next) => *next,
                Decision::Replay(_) => {
                    let audit = audit_collection(store)
                        .find_one(doc! {"_id":records::audit_key(&request.accepted, 1)})
                        .session(&mut *session)
                        .await?
                        .ok_or(Domain::Corrupt)?;
                    let original = records::outcome(&audit, &request.accepted)?;
                    if original.record_revision != 1 {
                        return Err(Domain::Corrupt.into());
                    }
                    return Ok(Receipt {
                        state: original,
                        audit: None,
                    });
                }
            }
        }
        Change::Progress(request) => {
            let desired = plan_pre_exclusion_progress(
                policy,
                &request.expected,
                &request.expected_source,
                request,
            )
            .map_err(Domain::from)?;
            let audit = records::audit(Some(&request.expected), &desired, &request.caller)?;
            let present = audit_collection(store)
                .find_one(doc! {"_id":audit.get("_id").ok_or(Domain::Corrupt)?})
                .session(&mut *session)
                .await?;
            if present.as_ref() == Some(&audit) {
                return Ok(Receipt {
                    state: desired,
                    audit: None,
                });
            }
            plan_pre_exclusion_progress(
                policy,
                existing.ok_or_else(|| refused(Refusal::Conflict))?,
                source,
                request,
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
    for (collection, row) in &snapshot.children {
        let result = collection
            .update_one(
                row.clone(),
                doc! {"$inc":{"ledger_maintenance_fence_v1":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(refused(Refusal::Conflict));
        }
    }
    let result = store.sandboxes.update_one(snapshot.parent, doc! {
        "$set":{records::FIELD:records::encode(&next)?, "document_schema_version":records::PARENT_VERSION},
        "$inc":{"ledger_maintenance_fence_v1":1_i64},
    }).session(&mut *session).await?;
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

pub(super) fn audit_collection(store: &MongoSandboxStore) -> mongodb::Collection<Document> {
    store
        .client
        .database(&store.sandboxes.namespace().db)
        .collection(records::AUDITS)
}
