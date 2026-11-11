//! Exact writer integration seam; call inside the existing writer transaction.

use layer2_core::sandbox::ledger_maintenance::{
    check_ledger_writer, LedgerMaintenanceObservation, LedgerWriterClass, LedgerWriterFence,
};
use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceAdmissionRefusal;
use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as Domain;
use layer2_core::SandboxId;
use mongodb::{bson::doc, ClientSession};

use super::ledger_maintenance::{refused, ApplyError};
use super::ledger_maintenance_records as records;
use super::sandbox_mongo::MongoSandboxStore;

/// A must integrate this into every existing admission/worker path. It grants
/// neither user authority nor provider exclusion. Already admitted external
/// work still requires the E2/E3 drain and physical exclusion protocol.
///
/// This guard has no flag bypass: disabling new maintenance admission cannot
/// release an existing hold. Missing parent, malformed marker and failed reads
/// refuse. The real parent write closes the absent-child/absent-lane race.
pub(super) async fn fence_writer(
    store: &MongoSandboxStore,
    sandbox: &SandboxId,
    class: LedgerWriterClass,
    fence: Option<LedgerWriterFence<'_>>,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":sandbox.as_str()})
        .session(&mut *session)
        .await?
        .ok_or_else(|| refused(LedgerMaintenanceAdmissionRefusal::NotFound))?;
    if let Some(preparation) = super::ledger_maintenance_pre_exclusion_records::read(&parent)? {
        let accepted = preparation.accepted().map_err(|_| Domain::Corrupt)?;
        let audit = super::ledger_maintenance_pre_exclusion_transaction::audit_collection(store)
            .find_one(doc! {"_id":super::ledger_maintenance_pre_exclusion_records::audit_key(&accepted, preparation.record_revision)})
            .session(&mut *session)
            .await?
            .ok_or(Domain::Corrupt)?;
        if super::ledger_maintenance_pre_exclusion_records::outcome(&audit, &accepted)?
            != preparation
        {
            return Err(Domain::Corrupt.into());
        }
        preparation.check_writer(class).map_err(Domain::from)?;
    }
    let actual = super::sandboxes::read_sandbox(&parent).map_err(|_| Domain::Corrupt)?;
    if actual.id != *sandbox {
        return Err(Domain::Corrupt.into());
    }
    let state = records::read(&parent)?;
    if let Some(state) = &state {
        if state.record.binding.sandbox_id != sandbox.as_str() {
            return Err(Domain::Corrupt.into());
        }
        let audit = super::ledger_maintenance_transaction::audit_collection(store)
            .find_one(doc! {"_id":records::state_hash(state)?})
            .session(&mut *session)
            .await?
            .ok_or(Domain::Corrupt)?;
        records::validate_audit(&audit, state)?;
    }
    check_ledger_writer(
        state
            .as_ref()
            .map_or(LedgerMaintenanceObservation::KnownAbsent, |state| {
                LedgerMaintenanceObservation::Present(&state.record)
            }),
        class,
        fence,
    )
    .map_err(Domain::from)?;
    // This fence remains in the accepted source digest. An admitted writer with
    // no aggregate change still invalidates a plan captured before that writer.
    let result = store
        .sandboxes
        .update_one(
            parent,
            doc! {"$inc":{"ledger_writer_admission_fence_v1":1_i64}},
        )
        .session(session)
        .await?;
    if result.matched_count != 1 {
        return Err(refused(LedgerMaintenanceAdmissionRefusal::Conflict));
    }
    Ok(())
}
