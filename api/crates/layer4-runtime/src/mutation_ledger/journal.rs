use rusqlite::{params, Connection, OptionalExtension};

use super::{budget, invalid, processes, storage_error};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_journal::{Operation, Record, Status};

pub(crate) struct Entry {
    pub(crate) record: Record,
    pub(crate) new: bool,
}

pub(super) fn scope(config: &RuntimeConfig, operation: &Operation) -> [u8; 8] {
    crate::mutation_scope::generation(config, &operation.subject)
        .unwrap_or(0)
        .to_be_bytes()
}

pub(super) fn reserve(
    db: &Connection,
    config: &RuntimeConfig,
    operation: &Operation,
) -> Result<bool> {
    if load(db, config, operation)?.is_some() {
        return Ok(false);
    }
    let identity = serde_json::to_vec(operation).map_err(|_| invalid())?;
    if identity.len() > 65536 {
        return Err(invalid());
    }
    let scope = scope(config, operation);
    let normal = budget::reserve(db, &scope, operation)?;
    db.execute(
        "INSERT INTO operations VALUES (?,?,?,0,?,NULL)",
        params![scope.as_slice(), operation.operation_id, identity, normal],
    )
    .map_err(|_| storage_error())?;
    Ok(normal)
}

pub(super) struct Stored {
    pub(super) record: Option<Record>,
    normal: bool,
}

pub(super) fn load_id(
    db: &Connection,
    config: &RuntimeConfig,
    generation_scope: &[u8; 8],
    operation_id: &str,
) -> Result<Option<Record>> {
    let identity: Option<Vec<u8>> = db
        .query_row(
            "SELECT identity FROM operations WHERE scope=? AND operation_id=? AND status>0",
            params![generation_scope.as_slice(), operation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| storage_error())?;
    let Some(identity) = identity else {
        return Ok(None);
    };
    if identity.len() > 65536 {
        return Err(invalid());
    }
    let operation: Operation = serde_json::from_slice(&identity).map_err(|_| invalid())?;
    if operation.operation_id != operation_id || scope(config, &operation) != *generation_scope {
        return Err(invalid());
    }
    let record = load(db, config, &operation)?.and_then(|stored| stored.record);
    Ok(Some(record.ok_or_else(invalid)?))
}

pub(super) fn load(
    db: &Connection,
    config: &RuntimeConfig,
    operation: &Operation,
) -> Result<Option<Stored>> {
    type Row = (Vec<u8>, i64, bool, Option<Vec<u8>>);
    let row: Option<Row> = db
        .query_row(
            "SELECT identity,status,normal,record FROM operations WHERE scope=? AND operation_id=?",
            params![scope(config, operation).as_slice(), operation.operation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(|_| storage_error())?;
    let Some((identity, status, normal, bytes)) = row else {
        return Ok(None);
    };
    if identity.len() > 65536 {
        return Err(invalid());
    }
    let stored: Operation = serde_json::from_slice(&identity).map_err(|_| invalid())?;
    if stored != *operation {
        return Err(crate::failure::RuntimeError::rejected(
            "runtime_mutation_replay_mismatch",
            "runtime mutation replay differs from the admitted request or rendered payload",
        ));
    }
    let record = bytes.map(|b| Record::decode(&b, config)).transpose()?;
    if let Some(record) = &record {
        record.exact(operation)?;
        if status != status_number(record.status()) {
            return Err(invalid());
        }
    } else if status != 0 {
        return Err(invalid());
    }
    Ok(Some(Stored { record, normal }))
}

pub(super) fn begin(
    db: &Connection,
    config: &RuntimeConfig,
    operation: &Operation,
) -> Result<Entry> {
    let stored = load(db, config, operation)?.ok_or_else(invalid)?;
    if let Some(record) = stored.record {
        return Ok(Entry { record, new: false });
    }
    let record = Record::started(config, operation)?;
    db.execute(
        "UPDATE operations SET status=1,record=? WHERE scope=? AND operation_id=? AND status=0",
        params![
            record.encode()?,
            scope(config, operation).as_slice(),
            operation.operation_id
        ],
    )
    .map_err(|_| storage_error())?;
    processes::update(db, config, &record)?;
    Ok(Entry { record, new: true })
}

pub(super) fn finish(db: &Connection, config: &RuntimeConfig, record: &Record) -> Result<()> {
    record.validate(config)?;
    if record.status() == Status::Started {
        return Err(invalid());
    }
    let operation = record.operation();
    let stored = load(db, config, &operation)?.ok_or_else(invalid)?;
    let prior = stored.record.ok_or_else(invalid)?;
    if prior.status() != Status::Started {
        return if prior == *record {
            Ok(())
        } else {
            Err(invalid())
        };
    }
    let scope = scope(config, &operation);
    db.execute(
        "UPDATE operations SET status=?,record=? WHERE scope=? AND operation_id=? AND status=1",
        params![
            status_number(record.status()),
            record.encode()?,
            scope.as_slice(),
            operation.operation_id
        ],
    )
    .map_err(|_| storage_error())?;
    processes::update(db, config, record)?;
    budget::finish(db, &scope, stored.normal)
}

fn status_number(status: Status) -> i64 {
    match status {
        Status::Started => 1,
        Status::Completed => 2,
        Status::Failed => 3,
        Status::Indeterminate => 4,
    }
}
