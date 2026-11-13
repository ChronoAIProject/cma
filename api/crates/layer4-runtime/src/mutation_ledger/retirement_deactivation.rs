//! Held deactivation. Retained source context plus the atomic accounting state
//! provide a reproducible receipt after loss of the original commit reply.
use rusqlite::{params, TransactionBehavior};
use sha2::Digest;

use super::{
    invalid, scan, source_check, storage_error, Auxiliary, Context, Ledger, Result, Selection,
    Sha256, SourceRow,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DeactivationReceipt {
    pub(crate) source_sha256: String,
    pub(crate) before_sha256: String,
    pub(crate) after_sha256: String,
    pub(crate) operations: u64,
    pub(crate) authority: u64,
    pub(crate) cleanup: u64,
    pub(crate) before_revision: u64,
    pub(crate) after_revision: u64,
    pub(crate) free_pages_before: u64,
    pub(crate) free_pages_after: u64,
}

impl Ledger {
    #[cfg(test)]
    pub(crate) async fn test_retirement_storage<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.storage.call(work).await
    }

    /// Called only by the owned archive/exclusion coordinator. The archive stays
    /// pending throughout this operation and the source store is closed afterward.
    pub(crate) async fn deactivate_retirement(
        &self,
        selection: Selection,
        owned_exclusion_and_archive: impl Send + 'static,
    ) -> Result<DeactivationReceipt> {
        self.verify()?;
        let config = self.config.clone();
        let guard = self.guard.clone();
        self.storage.call(move |db| {
            let _guard = guard;
            let _owned_exclusion_and_archive = owned_exclusion_and_archive;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|_| storage_error())?;
            let current = Context::read(&tx, &config)?;
            let after_accounting = selection.after_accounting()?;
            let original = current == selection.context;
            if !original && !after_matches(&selection.context, &current, after_accounting) {
                return Err(invalid());
            }
            let (count, digest) = scan::group(&tx, &config, &current, &selection)?;
            let expected_count = (selection.operations + selection.authority + selection.cleanup) as u64;
            if (original && (count != expected_count || digest != selection.group_digest)) ||
                (!original && count != 0) { return Err(invalid()); }
            // Shared scope rows must still match on both initial execution and retry.
            for row in &selection.rows {
                if (original || matches!(row, SourceRow::Auxiliary(Auxiliary::Scope { .. }))) &&
                    !source_check::matches(&tx, row)? { return Err(invalid()); }
            }
            if original {
                for row in &selection.rows {
                    let (scope, key) = row.key();
                    let sql = match row {
                        SourceRow::Operation { .. } => "DELETE FROM operations WHERE scope=? AND operation_id=?",
                        SourceRow::Authority { .. } => "DELETE FROM authority WHERE scope=? AND resource=?",
                        SourceRow::Auxiliary(Auxiliary::Cleanup { .. }) => "DELETE FROM cleanup_budget WHERE scope=? AND resource=?",
                        SourceRow::Auxiliary(Auxiliary::Scope { .. }) => continue,
                        SourceRow::Auxiliary(Auxiliary::Process { .. }) => return Err(invalid()),
                    };
                    if tx.execute(sql, params![scope, key]).map_err(|_| storage_error())? != 1 { return Err(invalid()); }
                }
                if tx.execute("UPDATE accounting SET revision=?,reserved=?,operations=?,authority=?,terminal=? WHERE singleton=1 AND revision=?",
                    params![after_accounting[0],after_accounting[1],after_accounting[2],after_accounting[3],after_accounting[4],selection.context.accounting[0]])
                    .map_err(|_| storage_error())? != 1 { return Err(invalid()); }
            }
            let after = Context::read(&tx, &config)?;
            if !after_matches(&selection.context, &after, after_accounting) ||
                scan::group(&tx, &config, &after, &selection)?.0 != 0 { return Err(invalid()); }
            let receipt = DeactivationReceipt {
                source_sha256: selection.proof.source_sha256,
                before_sha256: digest_context(&selection.context),
                after_sha256: digest_context(&after),
                operations: selection.operations as u64,
                authority: selection.authority as u64,
                cleanup: selection.cleanup as u64,
                before_revision: selection.context.accounting[0] as u64,
                after_revision: after.accounting[0] as u64,
                free_pages_before: selection.context.geometry[2] as u64,
                free_pages_after: after.geometry[2] as u64,
            };
            tx.commit().map_err(|_| storage_error())?;
            Ok(receipt)
        }).await
    }
}

fn after_matches(before: &Context, after: &Context, accounting: [i64; 5]) -> bool {
    let mut expected = before.clone();
    expected.accounting = accounting;
    // DELETE reuses pages through SQLite's freelist, without VACUUM or changing
    // the configured page ceiling. Geometry is evidence, not promised disk space.
    expected.geometry[2] = after.geometry[2];
    expected == *after && after.geometry[2] >= before.geometry[2]
}

fn digest_context(context: &Context) -> String {
    let mut digest = Sha256::new();
    context.hash_into(&mut digest);
    format!("{:x}", digest.finalize())
}
