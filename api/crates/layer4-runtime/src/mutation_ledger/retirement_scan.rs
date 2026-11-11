//! One bounded, consistent audit includes unselected rows and shared processes.
use rusqlite::Connection;
use sha2::Digest;

use super::selection::Selection;
use super::{
    accounting::Audit, invalid, page, Auxiliary, Context, Kind, Result, RuntimeConfig, Sha256,
    SourceRow, BYTES,
};

pub(super) fn group(
    db: &Connection,
    config: &RuntimeConfig,
    context: &Context,
    selection: &Selection,
) -> Result<(u64, String)> {
    let mut audit = Audit::default();
    let mut kind = Kind::Operations;
    let mut after = (Vec::new(), String::new());
    let mut scanned = 0_u64;
    let mut bytes = 0_usize;
    let mut selected = 0;
    let mut digest = Sha256::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(invalid());
        }
        let (rows, more) = page(db, config, kind, &after, BYTES - context.bytes())?;
        for row in &rows {
            scanned += 1;
            bytes = bytes.checked_add(row.bytes()).ok_or_else(invalid)?;
            if scanned > 262_144 || bytes > 256 * 1024 * 1024 {
                return Err(invalid());
            }
            audit.observe(db, row)?;
            if let SourceRow::Auxiliary(Auxiliary::Process { cwd, .. }) = row {
                if selection.worktrees.contains(cwd) {
                    return Err(invalid());
                }
            }
            if row.group()?.as_deref() == Some(selection.proof.key.as_str()) {
                row.hash_into(&mut digest);
                selected += 1;
            }
        }
        if more {
            let (scope, key) = rows.last().ok_or_else(invalid)?.key();
            after = (scope.into(), key.into());
        } else {
            audit.end_table(db, kind)?;
            let Some(next) = kind.next() else {
                break;
            };
            kind = next;
            after = (Vec::new(), String::new());
        }
    }
    audit.finish(context)?;
    Ok((selected, format!("{:x}", digest.finalize())))
}
