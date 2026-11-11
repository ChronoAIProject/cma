use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::{hash, invalid, storage_error, Context, GroupProof, Receipt, Result, SourceRow};
use crate::config::RuntimeConfig;

pub(super) fn group(
    db: &Connection,
    config: &RuntimeConfig,
    proof: &GroupProof,
    receipt: &Receipt,
    expected_context: Option<&Context>,
) -> Result<Context> {
    let (size, expected, payload): (i64, String, Vec<u8>) = db.query_row(
        "SELECT length(payload),sha256,CASE WHEN length(payload)<=2097152 THEN payload ELSE NULL END
         FROM indexed_contexts WHERE group_key=?", [&proof.key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|_| invalid())?;
    if !(1..=2 * 1024 * 1024).contains(&size) || hash(&payload) != expected {
        return Err(invalid());
    }
    let context = Context::decode(&payload, config)?;
    if expected_context.is_some_and(|expected| expected != &context) {
        return Err(invalid());
    }
    let config = context.scoped_config(config)?;
    let legacy: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM archived_records WHERE group_key=?)",
            [&proof.key],
            |r| r.get(0),
        )
        .map_err(|_| invalid())?;
    if legacy {
        return Err(invalid());
    }
    let mut source = Sha256::new();
    context.hash_into(&mut source);
    let mut count = 0;
    let mut bytes = payload.len() as u64;
    let mut operations = 0;
    let mut authority = 0;
    let mut statement = db.prepare(
        "SELECT kind,scope,source_key,sha256,length(payload),CASE WHEN length(payload)<=8388608 THEN payload ELSE NULL END
         FROM indexed_records WHERE group_key=? ORDER BY kind,scope,source_key")
        .map_err(|_| storage_error())?;
    let mut records = statement.query([&proof.key]).map_err(|_| storage_error())?;
    while let Some(record) = records.next().map_err(|_| storage_error())? {
        if count >= receipt.records {
            return Err(invalid());
        }
        let size: i64 = record.get(4).map_err(|_| invalid())?;
        if !(1..=8 * 1024 * 1024).contains(&size)
            || bytes
                .checked_add(size as u64)
                .is_none_or(|next| next > receipt.bytes)
        {
            return Err(invalid());
        }
        let payload: Vec<u8> = record.get(5).map_err(|_| invalid())?;
        let expected: String = record.get(3).map_err(|_| invalid())?;
        if hash(&payload) != expected {
            return Err(invalid());
        }
        let row = SourceRow::decode(&payload, &config)?;
        let ordering: (u8, Vec<u8>, String) = (
            record.get(0).map_err(|_| invalid())?,
            record.get(1).map_err(|_| invalid())?,
            record.get(2).map_err(|_| invalid())?,
        );
        if row.ordering() != ordering || row.group()?.is_some_and(|key| key != proof.key) {
            return Err(invalid());
        }
        operations += u32::from(ordering.0 == 3);
        authority += u32::from(ordering.0 == 4);
        for operation in row.operation_ids()? {
            let owner: Option<String> = db
                .query_row(
                    "SELECT group_key FROM retired_operations WHERE scope=? AND operation_id=?",
                    params![row.key().0, operation],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|_| invalid())?;
            if owner.as_deref() != Some(proof.key.as_str()) {
                return Err(invalid());
            }
        }
        row.hash_into(&mut source);
        count += 1;
        bytes = bytes
            .checked_add(payload.len() as u64)
            .ok_or_else(invalid)?;
    }
    if operations == 0
        || authority == 0
        || count != receipt.records
        || bytes != receipt.bytes
        || format!("{:x}", source.finalize()) != proof.source_sha256
    {
        return Err(invalid());
    }
    Ok(context)
}

/// The caller holds one read transaction across verification and visitation.
/// No callback sees a prefix before the complete archive and replay indexes pass.
pub(super) fn visit(
    db: &Connection,
    config: &RuntimeConfig,
    proof: &GroupProof,
    receipt: &Receipt,
    mut inspect: impl FnMut(&Context, SourceRow) -> Result<()>,
) -> Result<()> {
    let context = group(db, config, proof, receipt, None)?;
    let scoped = context.scoped_config(config)?;
    let mut statement = db
        .prepare(
            "SELECT payload FROM indexed_records WHERE group_key=? ORDER BY kind,scope,source_key",
        )
        .map_err(|_| storage_error())?;
    let mut rows = statement.query([&proof.key]).map_err(|_| storage_error())?;
    while let Some(row) = rows.next().map_err(|_| storage_error())? {
        let bytes: Vec<u8> = row.get(0).map_err(|_| invalid())?;
        inspect(&context, SourceRow::decode(&bytes, &scoped)?)?;
    }
    Ok(())
}
