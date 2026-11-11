//! Qualification-only row/accounting primitive, with no commit or public caller.
//! A must record the exact accepted restore operation/outcome in this transaction
//! before committing. C's coordinator must retain the verified archive, durable
//! non-serving hold, exclusion, backup and independent no-newer-effects proof.
use rusqlite::{params, Transaction};

use super::{changed, compare_source, digest, scan, source_check};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_ledger::{
    retirement_inventory::{Auxiliary, Context, DeactivationReceipt, Selection, SourceRow},
    storage_error,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(super) struct RestoredSource {
    pub(super) format: &'static str,
    pub(super) source_sha256: String,
    pub(super) before_sha256: String,
    pub(super) after_sha256: String,
    pub(super) before_revision: u64,
    pub(super) after_revision: u64,
    pub(super) operations: u64,
    pub(super) authority: u64,
    pub(super) cleanup: u64,
}

/// Does not commit. Errors require rollback of the enclosing transaction. Repeat
/// requests must resolve through A's durable outcome, never invoke this again or
/// infer a new operation from the current aggregate counters.
pub(super) fn restore_rows(
    tx: &Transaction<'_>,
    config: &RuntimeConfig,
    original: &Selection,
    transition: &DeactivationReceipt,
) -> Result<RestoredSource> {
    let before = compare_source(tx, config, original, transition)?;
    let mut expected = before.clone();
    expected.accounting = original.context.accounting;
    // Preserve maintenance history; only the selected data/counters return.
    expected.accounting[0] = before.accounting[0].checked_add(1).ok_or_else(changed)?;
    for row in &original.rows {
        insert(tx, row)?;
    }
    if tx.execute(
        "UPDATE accounting SET revision=?,reserved=?,operations=?,authority=?,terminal=?
         WHERE singleton=1 AND revision=? AND reserved=? AND operations=? AND authority=? AND terminal=?",
        params![expected.accounting[0], expected.accounting[1], expected.accounting[2],
            expected.accounting[3], expected.accounting[4], before.accounting[0],
            before.accounting[1], before.accounting[2], before.accounting[3], before.accounting[4]],
    ).map_err(|_| storage_error())? != 1 {
        return Err(changed());
    }
    let after = Context::read(tx, config)?;
    // Reallocated pages are measured, not rewound; the page size/ceiling stay fixed.
    expected.geometry[1] = after.geometry[1];
    expected.geometry[2] = after.geometry[2];
    let (count, group_digest) = scan::group(tx, config, &after, original)?;
    if after != expected
        || count != (original.operations + original.authority + original.cleanup) as u64
        || group_digest != original.group_digest
    {
        return Err(changed());
    }
    for row in &original.rows {
        if !source_check::matches(tx, row)? {
            return Err(changed());
        }
    }
    Ok(RestoredSource {
        format: "runtime-ledger-restored-source-v1",
        source_sha256: original.proof.source_sha256.clone(),
        before_sha256: digest(&before),
        after_sha256: digest(&after),
        before_revision: before.accounting[0] as u64,
        after_revision: after.accounting[0] as u64,
        operations: original.operations as u64,
        authority: original.authority as u64,
        cleanup: original.cleanup as u64,
    })
}

fn insert(tx: &Transaction<'_>, row: &SourceRow) -> Result<()> {
    // INSERT only: no replacement, conflict-ignore, deletion or partial merging.
    let count = match row {
        SourceRow::Operation { scope, id, identity, status, normal, record } => tx.execute(
            "INSERT INTO operations(scope,operation_id,identity,status,normal,record) VALUES(?,?,?,?,?,?)",
            params![scope, id, identity, status, normal, record]),
        SourceRow::Authority { scope, resource, high, records } => tx.execute(
            "INSERT INTO authority(scope,resource,high_water,present,first_absent,last_absent) VALUES(?,?,?,?,?,?)",
            params![scope, resource, high, records[0], records[1], records[2]]),
        SourceRow::Auxiliary(Auxiliary::Cleanup { scope, resource, available }) => tx.execute(
            "INSERT INTO cleanup_budget(scope,resource,available) VALUES(?,?,?)", params![scope, resource, available]),
        SourceRow::Auxiliary(Auxiliary::Scope { .. }) => return Ok(()),
        SourceRow::Auxiliary(Auxiliary::Process { .. }) => return Err(changed()),
    }.map_err(|_| storage_error())?;
    if count != 1 {
        return Err(changed());
    }
    Ok(())
}
