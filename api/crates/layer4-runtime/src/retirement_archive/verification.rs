//! Verify one committed group without loading its byte history into memory.
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::{group_key, invalid, storage_error, GroupProof, Receipt, SourceRecord};
use crate::config::RuntimeConfig;
use crate::failure::Result;

pub(super) fn group(
    db: &Connection,
    config: &RuntimeConfig,
    proof: &GroupProof,
    receipt: &Receipt,
) -> Result<()> {
    visit(db, config, proof, receipt, |_| Ok(()))
}

pub(super) fn visit(
    db: &Connection,
    config: &RuntimeConfig,
    proof: &GroupProof,
    receipt: &Receipt,
    mut consume: impl FnMut(&SourceRecord) -> Result<()>,
) -> Result<()> {
    let mut statement = db.prepare(
        "SELECT scope,kind,source_name,sha256,payload FROM archived_records WHERE group_key=? ORDER BY scope,kind,source_name",
    ).map_err(|_| storage_error())?;
    let mut rows = statement.query([&proof.key]).map_err(|_| storage_error())?;
    let mut digest = Sha256::new();
    let mut records = 0_u64;
    let mut bytes = 0_u64;
    while let Some(row) = rows.next().map_err(|_| storage_error())? {
        if records >= receipt.records {
            return Err(invalid());
        }
        let scope: Vec<u8> = row.get(0).map_err(|_| invalid())?;
        let record = SourceRecord {
            scope: u64::from_be_bytes(scope.try_into().map_err(|_| invalid())?),
            kind: row.get(1).map_err(|_| invalid())?,
            name: row.get(2).map_err(|_| invalid())?,
            bytes: row.get(4).map_err(|_| invalid())?,
        };
        let expected: String = row.get(3).map_err(|_| invalid())?;
        if format!("{:x}", Sha256::digest(&record.bytes)) != expected {
            return Err(invalid());
        }
        let operation = record.operation(config)?;
        if group_key(&operation) != proof.key {
            return Err(invalid());
        }
        let owner: Option<String> = db
            .query_row(
                "SELECT group_key FROM retired_operations WHERE scope=? AND operation_id=?",
                params![
                    record.scope.to_be_bytes().as_slice(),
                    operation.operation_id
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage_error())?;
        if owner.as_deref() != Some(proof.key.as_str()) {
            return Err(invalid());
        }
        consume(&record)?;
        record.hash_into(&mut digest);
        records += 1;
        bytes = bytes
            .checked_add(record.bytes.len() as u64)
            .ok_or_else(invalid)?;
    }
    if records != receipt.records
        || bytes != receipt.bytes
        || format!("{:x}", digest.finalize()) != proof.source_sha256
    {
        return Err(invalid());
    }
    Ok(())
}
