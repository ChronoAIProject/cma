//! Exact bytes use bounded chunks: an allowed 4 MiB observation must not exceed
//! the unchanged 2 MiB SQLite value/row limit. All chunks share the intent txn.
use super::{invalid, storage_error};
use crate::failure::Result;
use rusqlite::{params, Connection};
const CHUNK: usize = 65_536;
const LIMITS: [usize; 3] = [2 * 1024 * 1024, 65_536, 4 * 1024 * 1024];

pub(super) fn put(db: &Connection, operation: &str, originals: &[Vec<u8>; 3]) -> Result<()> {
    for (kind, bytes) in originals.iter().enumerate() {
        if bytes.is_empty() || bytes.len() > LIMITS[kind] {
            return Err(invalid());
        }
        for (ordinal, chunk) in bytes.chunks(CHUNK).enumerate() {
            db.execute(
                "INSERT INTO collection_artifacts VALUES (?,?,?,?)",
                params![operation, kind as u32, ordinal as u32, chunk],
            )
            .map_err(|_| storage_error())?;
        }
    }
    Ok(())
}

pub(super) fn read(db: &Connection, operation: &str) -> Result<[Vec<u8>; 3]> {
    let sizes:[u32;3]=db.query_row(
        "SELECT candidate_bytes,request_bytes,observation_bytes FROM collection_evidence WHERE operation_id=?",
        [operation],|r| Ok([r.get(0)?,r.get(1)?,r.get(2)?])).map_err(|_| invalid())?;
    let sizes = sizes.map(|size| size as usize);
    if sizes
        .iter()
        .zip(LIMITS)
        .any(|(size, limit)| !(1..=limit).contains(size))
    {
        return Err(invalid());
    }
    let mut output = sizes.map(Vec::with_capacity);
    let mut query=db.prepare("SELECT kind,ordinal,payload FROM collection_artifacts WHERE operation_id=? ORDER BY kind,ordinal LIMIT 98")
        .map_err(|_| invalid())?;
    let mut rows = query.query([operation]).map_err(|_| invalid())?;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|_| invalid())? {
        count += 1;
        let kind = row.get::<_, u32>(0).map_err(|_| invalid())? as usize;
        let ordinal = row.get::<_, u32>(1).map_err(|_| invalid())? as usize;
        let bytes: Vec<u8> = row.get(2).map_err(|_| invalid())?;
        let Some(target) = output.get_mut(kind) else {
            return Err(invalid());
        };
        if count > 97
            || ordinal.checked_mul(CHUNK) != Some(target.len())
            || bytes.len() != CHUNK.min(sizes[kind] - target.len())
            || bytes.is_empty()
        {
            return Err(invalid());
        }
        target.extend_from_slice(&bytes);
    }
    if output.each_ref().map(Vec::len) != sizes {
        return Err(invalid());
    }
    Ok(output)
}
