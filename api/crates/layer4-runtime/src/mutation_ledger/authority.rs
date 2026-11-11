use rusqlite::{params, Connection, OptionalExtension};

use super::{fence, invalid, storage_error};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::{self as v1, Disposition, Record, ResourceKey};
use crate::mutation_journal::{Operation, Subject};

pub(super) fn admit(
    db: &Connection,
    config: &RuntimeConfig,
    fence: u64,
    operation: &Operation,
) -> Result<()> {
    let (resource, _) = v1::resource_operation(operation);
    if let Subject::InstallAgentSkill { request } = &operation.subject {
        let parent = ResourceKey::AgentHome {
            agent_id: request.agent_id.as_str().into(),
        };
        v1::require_skill_parent(&load(db, config, &parent, &[0; 8])?, operation)?;
    }
    let scope = crate::mutation_scope::generation(config, &operation.subject)
        .unwrap_or(0)
        .to_be_bytes();
    let mut records = load(db, config, &resource, &scope)?;
    let incoming = Record::new(config, fence, operation);
    if !v1::authorize(&records.iter().collect::<Vec<_>>(), &incoming)? {
        return Ok(());
    }
    if records.is_empty() {
        db.execute(
            "UPDATE accounting SET authority=authority+1 WHERE singleton=1",
            [],
        )
        .map_err(|_| storage_error())?;
    }
    records.push(incoming);
    let present = records
        .iter()
        .filter(|r| r.disposition == Disposition::Present)
        .max_by_key(|r| r.authority_fence);
    let first_absent = records
        .iter()
        .filter(|r| r.disposition == Disposition::Absent)
        .min_by_key(|r| r.authority_fence);
    let last_absent = records
        .iter()
        .filter(|r| r.disposition == Disposition::Absent)
        .max_by_key(|r| r.authority_fence);
    let encode = |record: Option<&Record>| -> Result<Option<Vec<u8>>> {
        record
            .map(|record| {
                let bytes = serde_json::to_vec(record).map_err(|_| invalid())?;
                if bytes.len() > v1::MAX_AUTHORITY_BYTES as usize {
                    return Err(invalid());
                }
                Ok(bytes)
            })
            .transpose()
    };
    db.execute("INSERT INTO authority VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(scope,resource) DO UPDATE SET high_water=excluded.high_water, present=excluded.present, first_absent=excluded.first_absent, last_absent=excluded.last_absent",
        params![scope.as_slice(), key(&resource)?, fence::encode(fence).as_slice(), encode(present)?, encode(first_absent)?, encode(last_absent)?])
        .map_err(|_| storage_error())?;
    Ok(())
}

pub(super) fn key(resource: &ResourceKey) -> Result<String> {
    serde_json::to_string(resource).map_err(|_| invalid())
}

pub(super) fn load(
    db: &Connection,
    config: &RuntimeConfig,
    resource: &ResourceKey,
    scope: &[u8; 8],
) -> Result<Vec<Record>> {
    type Stored = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
    let row: Option<Stored> = db.query_row(
        "SELECT high_water,present,first_absent,last_absent FROM authority WHERE scope=? AND resource=?",
        params![scope.as_slice(), key(resource)?],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional().map_err(|_| storage_error())?;
    let Some((high, present, first, last)) = row else {
        return Ok(Vec::new());
    };
    decode(
        config,
        resource,
        scope,
        &high,
        [present.as_deref(), first.as_deref(), last.as_deref()],
    )
}

pub(super) fn decode(
    config: &RuntimeConfig,
    resource: &ResourceKey,
    scope: &[u8; 8],
    high: &[u8],
    blobs: [Option<&[u8]>; 3],
) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let mut absent_fences = Vec::new();
    for (bytes, disposition) in [
        (blobs[0], Disposition::Present),
        (blobs[1], Disposition::Absent),
        (blobs[2], Disposition::Absent),
    ] {
        if let Some(bytes) = bytes {
            if bytes.len() > v1::MAX_AUTHORITY_BYTES as usize {
                return Err(invalid());
            }
            let record: Record = serde_json::from_slice(bytes).map_err(|_| invalid())?;
            let operation = record.operation();
            crate::mutation_journal::validate_operation(config, &operation)
                .map_err(|_| invalid())?;
            if record != Record::new(config, record.authority_fence, &operation)
                || record.resource != *resource
                || record.disposition != disposition
                || record.authority_fence == 0
                || crate::mutation_scope::generation(config, &operation.subject)
                    .unwrap_or(0)
                    .to_be_bytes()
                    != *scope
            {
                return Err(invalid());
            }
            if disposition == Disposition::Absent {
                absent_fences.push(record.authority_fence);
            }
            records.push(record);
        }
    }
    if absent_fences.len() == 1 || absent_fences.windows(2).any(|f| f[0] > f[1]) {
        return Err(invalid());
    }
    v1::validate_history(&records.iter().collect::<Vec<_>>()).map_err(|_| invalid())?;
    if records.iter().map(|r| r.authority_fence).max() != Some(fence::decode(high)?) {
        return Err(invalid());
    }
    Ok(records)
}
