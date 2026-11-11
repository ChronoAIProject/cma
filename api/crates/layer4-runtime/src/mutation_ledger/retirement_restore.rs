//! Read-only comparison for held restore planning. A matching source is necessary
//! but does not establish retained backup, exclusion, consent or no newer effects.

use sha2::{Digest, Sha256};

use super::{
    scan, source_check, Auxiliary, Context, DeactivationReceipt, Ledger, Result, Selection,
    SourceRow,
};
use crate::failure::RuntimeError;

#[cfg(test)]
#[path = "retirement_restore_transaction.rs"]
mod transaction;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RestoreSourceAssessment {
    pub(crate) format: &'static str,
    pub(crate) source_sha256: String,
    pub(crate) before_sha256: String,
    pub(crate) after_sha256: String,
    pub(crate) operations: u64,
    pub(crate) authority: u64,
    pub(crate) cleanup: u64,
    pub(crate) after_revision: u64,
    pub(crate) no_newer_effects_proof_required: bool,
    pub(crate) retained_backup_verification_required: bool,
    pub(crate) restoration_authorized: bool,
}

impl Ledger {
    /// The caller must load the exact original selection from a verified archive,
    /// including its permanent replay indexes. This does not construct authority
    /// from source contents and does not make a writable source or archive handle.
    pub(crate) async fn assess_retirement_restore(
        &self,
        original: Selection,
        transition: DeactivationReceipt,
    ) -> Result<RestoreSourceAssessment> {
        self.verify()?;
        validate_transition(&original, &transition)?;
        let config = self.config.clone();
        self.storage
            .call(move |db| {
                // Normalize all query/callback failures before returning to the
                // worker: inspection must never spend its emergency reserve.
                let tx = db.transaction().map_err(|_| changed())?;
                compare_source(&tx, &config, &original, &transition)?;
                tx.rollback().map_err(|_| changed())?;
                Ok(RestoreSourceAssessment {
                    format: "runtime-ledger-restore-source-assessment-v1",
                    source_sha256: transition.source_sha256,
                    before_sha256: transition.before_sha256,
                    after_sha256: transition.after_sha256,
                    operations: transition.operations,
                    authority: transition.authority,
                    cleanup: transition.cleanup,
                    after_revision: transition.after_revision,
                    no_newer_effects_proof_required: true,
                    retained_backup_verification_required: true,
                    restoration_authorized: false,
                })
            })
            .await
    }
}

fn compare_source(
    db: &rusqlite::Connection,
    config: &crate::config::RuntimeConfig,
    original: &Selection,
    transition: &DeactivationReceipt,
) -> Result<Context> {
    validate_transition(original, transition)?;
    let current = Context::read(db, config).map_err(|_| changed())?;
    let mut expected = original.context.clone();
    expected.accounting = original.after_accounting().map_err(|_| changed())?;
    expected.geometry[2] = i64::try_from(transition.free_pages_after).map_err(|_| changed())?;
    if current != expected
        || digest(&current) != transition.after_sha256
        || scan::group(db, config, &current, original)
            .map_err(|_| changed())?
            .0
            != 0
    {
        return Err(changed());
    }
    // Shared projections survived deactivation; do not recreate an old snapshot
    // over a changed row, even if aggregate accounting still matches.
    for row in &original.rows {
        if matches!(row, SourceRow::Auxiliary(Auxiliary::Scope { .. }))
            && !source_check::matches(db, row).map_err(|_| changed())?
        {
            return Err(changed());
        }
    }
    Ok(current)
}

fn validate_transition(original: &Selection, transition: &DeactivationReceipt) -> Result<()> {
    let after = original.after_accounting().map_err(|_| changed())?;
    let mut expected_after = original.context.clone();
    expected_after.accounting = after;
    expected_after.geometry[2] =
        i64::try_from(transition.free_pages_after).map_err(|_| changed())?;
    if transition.source_sha256 != original.proof.source_sha256
        || transition.before_sha256 != digest(&original.context)
        || transition.after_sha256 != digest(&expected_after)
        || transition.before_revision != original.context.accounting[0] as u64
        || transition.after_revision != after[0] as u64
        || transition.free_pages_before != original.context.geometry[2] as u64
        || transition.free_pages_after < transition.free_pages_before
        || transition.free_pages_after > original.context.geometry[1] as u64
        || transition.operations != original.operations as u64
        || transition.authority != original.authority as u64
        || transition.cleanup != original.cleanup as u64
    {
        return Err(changed());
    }
    Ok(())
}

fn digest(context: &Context) -> String {
    let mut digest = Sha256::new();
    context.hash_into(&mut digest);
    format!("{:x}", digest.finalize())
}

fn changed() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_restore_source_changed",
        "held restore requires the exact original transition and unchanged source; retain the hold",
    )
}

#[cfg(test)]
#[path = "retirement_restore_tests.rs"]
mod tests;
