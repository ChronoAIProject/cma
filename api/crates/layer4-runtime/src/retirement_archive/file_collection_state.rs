//! A bounded retained census gives V1 an explicit observed-source revision.
//! It cannot change while a collection intent is pending.
use super::{invalid, storage_error};
use crate::{failure::Result, ledger_import::collection_source::Snapshot};
use rusqlite::{params, Connection, OptionalExtension};
const CHUNK: usize = 65_536;

pub(super) fn read(db: &Connection) -> Result<Option<(u64, Snapshot)>> {
    let row: Option<(i64, String, i64)> = db
        .query_row(
            "SELECT revision,sha256,bytes FROM file_collection_source WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|_| invalid())?;
    let Some((revision, hash, length)) = row else {
        return Ok(None);
    };
    if revision <= 0 || !(1..=24 * 1024 * 1024).contains(&length) {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(length as usize);
    let mut query = db
        .prepare("SELECT ordinal,payload FROM file_collection_snapshot ORDER BY ordinal LIMIT 385")
        .map_err(|_| invalid())?;
    let mut rows = query.query([]).map_err(|_| invalid())?;
    while let Some(row) = rows.next().map_err(|_| invalid())? {
        let index: u32 = row.get(0).map_err(|_| invalid())?;
        let chunk: Vec<u8> = row.get(1).map_err(|_| invalid())?;
        if bytes.len() >= length as usize
            || index as usize * CHUNK != bytes.len()
            || chunk.len() != CHUNK.min(length as usize - bytes.len())
        {
            return Err(invalid());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != length as usize {
        return Err(invalid());
    }
    let snapshot = Snapshot::decode(&bytes)?;
    if snapshot.hash()? != hash {
        return Err(invalid());
    }
    Ok(Some((revision as u64, snapshot)))
}

pub(super) fn write(db: &Connection, revision: u64, snapshot: &Snapshot) -> Result<()> {
    let revision = i64::try_from(revision).map_err(|_| invalid())?;
    if revision <= 0 {
        return Err(invalid());
    }
    let bytes = snapshot.encode()?;
    db.execute("INSERT INTO file_collection_source VALUES (1,?,?,?) ON CONFLICT(singleton) DO UPDATE SET revision=excluded.revision,sha256=excluded.sha256,bytes=excluded.bytes",
        params![revision,snapshot.hash()?,bytes.len() as i64]).map_err(|_| storage_error())?;
    db.execute("DELETE FROM file_collection_snapshot", [])
        .map_err(|_| storage_error())?;
    for (ordinal, chunk) in bytes.chunks(CHUNK).enumerate() {
        db.execute(
            "INSERT INTO file_collection_snapshot VALUES (?,?)",
            params![ordinal as u32, chunk],
        )
        .map_err(|_| storage_error())?;
    }
    if read(db)? != Some((revision as u64, snapshot.clone())) {
        return Err(invalid());
    }
    Ok(())
}
