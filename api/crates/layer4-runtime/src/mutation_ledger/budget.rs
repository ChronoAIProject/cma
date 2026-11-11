//! Logical reservations supplement SQLite's page cap; they never promise disk health.
use rusqlite::{params, Connection, OptionalExtension};

use super::{authority, invalid, storage_error};
use crate::failure::Result;
use crate::mutation_authority::{Disposition, ResourceKey};
use crate::mutation_journal::Operation;

// Four times the largest outcome + identity, plus 64 pages for tree/index growth.
// Admission pairs have at most two entries; transaction/journal headroom is shared.
pub(super) const OUTCOME: i64 = 4 * (1024 * 1024 + 64 * 1024) + 64 * 4096;
pub(super) const HEADROOM: i64 = 16 * 1024 * 1024;
pub(super) const OUTSTANDING: i64 = 1024;

pub(super) fn reserve(db: &Connection, scope: &[u8; 8], operation: &Operation) -> Result<bool> {
    let (resource, disposition) = crate::mutation_authority::resource_operation(operation);
    let key = authority::key(&resource)?;
    let existing: Option<bool> = db
        .query_row(
            "SELECT available FROM cleanup_budget WHERE scope=? AND resource=?",
            params![scope.as_slice(), key],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    let covered = disposition == Disposition::Absent && existing == Some(true);
    let mut reserved = OUTCOME;
    if !matches!(resource, ResourceKey::AgentSkill { .. }) {
        if existing.is_none() {
            let available = disposition == Disposition::Present;
            db.execute(
                "INSERT INTO cleanup_budget VALUES (?,?,?)",
                params![scope.as_slice(), key, available],
            )
            .map_err(|_| storage_error())?;
            if available {
                reserved += OUTCOME;
            }
        } else if covered {
            db.execute(
                "UPDATE cleanup_budget SET available=0 WHERE scope=? AND resource=?",
                params![scope.as_slice(), key],
            )
            .map_err(|_| storage_error())?;
            reserved -= OUTCOME;
        }
    } else if let ResourceKey::AgentSkill { agent_id, .. } = resource {
        // A skill may predate the home journal. Reserve its parent's removal too.
        let parent = authority::key(&ResourceKey::AgentHome { agent_id })?;
        let inserted = db
            .execute(
                "INSERT INTO cleanup_budget VALUES (zeroblob(8),?,1) ON CONFLICT DO NOTHING",
                [parent],
            )
            .map_err(|_| storage_error())?;
        reserved += inserted as i64 * OUTCOME;
    }
    db.execute(
        "UPDATE accounting SET reserved=reserved+?,operations=operations+1 WHERE singleton=1",
        [reserved],
    )
    .map_err(|_| storage_error())?;
    if !covered {
        db.execute("INSERT INTO scope_budget VALUES (?,1,0) ON CONFLICT(scope) DO UPDATE SET pending=pending+1", [scope.as_slice()]).map_err(|_| storage_error())?;
    }
    Ok(!covered)
}

pub(super) fn finish(db: &Connection, scope: &[u8; 8], normal: bool) -> Result<()> {
    db.execute(
        "UPDATE accounting SET reserved=reserved-?,terminal=terminal+1 WHERE singleton=1",
        [OUTCOME],
    )
    .map_err(|_| storage_error())?;
    if normal
        && db
            .execute(
                "UPDATE scope_budget SET pending=pending-1 WHERE scope=? AND pending>0",
                [scope.as_slice()],
            )
            .map_err(|_| storage_error())?
            != 1
    {
        return Err(invalid());
    }
    // A missing scope row means zero in every admission/recovery reader. Keep
    // the derived working set tied to pending work or logical processes, not
    // the number of past generations. Receipts and authority stay retained.
    db.execute(
        "DELETE FROM scope_budget WHERE scope=? AND pending=0 AND processes=0",
        [scope.as_slice()],
    )
    .map_err(|_| storage_error())?;
    Ok(())
}

pub(super) fn check(db: &Connection, generation: u64) -> Result<()> {
    let reserved: i64 = db
        .query_row(
            "SELECT reserved FROM accounting WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| storage_error())?;
    let pending: i64 = db
        .query_row(
            "SELECT coalesce(sum(pending),0) FROM scope_budget WHERE scope IN (zeroblob(8),?)",
            [generation.to_be_bytes().as_slice()],
            |r| r.get(0),
        )
        .map_err(|_| storage_error())?;
    let pages: i64 = db
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(|_| storage_error())?;
    let maximum: i64 = db
        .pragma_query_value(None, "max_page_count", |r| r.get(0))
        .map_err(|_| storage_error())?;
    if pending > OUTSTANDING || pages * 4096 + reserved + HEADROOM > maximum * 4096 {
        return Err(crate::mutation_capacity::exhausted());
    }
    Ok(())
}

pub(super) fn revision(db: &Connection) -> Result<()> {
    db.execute(
        "UPDATE accounting SET revision=revision+1 WHERE singleton=1",
        [],
    )
    .map_err(|_| storage_error())?;
    Ok(())
}
