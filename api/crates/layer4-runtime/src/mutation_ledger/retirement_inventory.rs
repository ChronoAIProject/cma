//! Bounded, revision-bound source enumeration. These are source facts, never
//! retirement eligibility or permission to remove rows from the active store.
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{authority, fence, invalid, journal, storage_error, Ledger};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_authority::ResourceKey;
use crate::mutation_journal::{Operation, Record, Status};

#[path = "retirement_accounting.rs"]
mod accounting;
#[path = "retirement_auxiliary.rs"]
mod auxiliary;
#[path = "retirement_binary.rs"]
mod binary;
#[path = "retirement_context.rs"]
mod context;
#[path = "retirement_encoding.rs"]
mod encoding;
#[path = "retirement_source_check.rs"]
mod source_check;
pub(crate) use auxiliary::Auxiliary;
pub(crate) use context::Context;

const ROWS: usize = 64;
const BYTES: usize = 2 * 1024 * 1024;
const OPERATIONS: &str = "SELECT scope,operation_id,identity,status,normal,record,
    length(scope)+length(CAST(operation_id AS BLOB))+length(identity)+coalesce(length(record),0)
    FROM operations WHERE (scope,operation_id)>(?,?) ORDER BY scope,operation_id LIMIT 65";
const AUTHORITY: &str = "SELECT scope,resource,high_water,present,first_absent,last_absent,
    length(scope)+length(CAST(resource AS BLOB))+length(high_water)+coalesce(length(present),0)+coalesce(length(first_absent),0)+coalesce(length(last_absent),0)
    FROM authority WHERE (scope,resource)>(?,?) ORDER BY scope,resource LIMIT 65";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Operations,
    Authority,
    Cleanup,
    Scopes,
    Processes,
}

impl Kind {
    fn next(self) -> Option<Self> {
        match self {
            Self::Operations => Some(Self::Authority),
            Self::Authority => Some(Self::Cleanup),
            Self::Cleanup => Some(Self::Scopes),
            Self::Scopes => Some(Self::Processes),
            Self::Processes => None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Cursor {
    context: Context,
    audit: accounting::Audit,
    kind: Kind,
    after: (Vec<u8>, String),
}

/// Preserve every original logical column, including raw identity bytes and
/// reservation classification. Decoding a journal alone would lose this state.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "table", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SourceRow {
    Auxiliary(Auxiliary),
    Operation {
        scope: Vec<u8>,
        id: String,
        #[serde(with = "binary")]
        identity: Vec<u8>,
        status: i64,
        normal: bool,
        #[serde(with = "binary::optional")]
        record: Option<Vec<u8>>,
    },
    Authority {
        scope: Vec<u8>,
        resource: String,
        high: Vec<u8>,
        #[serde(with = "binary::folded")]
        records: [Option<Vec<u8>>; 3],
    },
}

pub(crate) struct Page {
    pub(crate) context: Context,
    pub(crate) rows: Vec<SourceRow>,
    pub(crate) next: Option<Cursor>,
}

impl SourceRow {
    pub(crate) fn key(&self) -> (&[u8], &str) {
        match self {
            Self::Operation { scope, id, .. } => (scope, id),
            Self::Auxiliary(row) => row.key(),
            Self::Authority {
                scope, resource, ..
            } => (scope, resource),
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        if let Self::Auxiliary(row) = self {
            return row.bytes();
        }
        let (scope, key) = self.key();
        scope.len()
            + key.len()
            + match self {
                Self::Auxiliary(_) => unreachable!(),
                Self::Operation {
                    identity, record, ..
                } => 9 + identity.len() + record.as_ref().map_or(0, Vec::len),
                Self::Authority { high, records, .. } => {
                    high.len() + records.iter().flatten().map(Vec::len).sum::<usize>()
                }
            }
    }

    /// Domain-separated, length-prefixed encoding includes NULL distinctions.
    /// It is a source comparison digest, not an archive or deletion receipt.
    pub(crate) fn hash_into(&self, digest: &mut Sha256) {
        if let Self::Auxiliary(row) = self {
            row.hash_into(digest);
            return;
        }
        let (scope, key) = self.key();
        digest.update([match self {
            Self::Auxiliary(_) => unreachable!(),
            Self::Operation { .. } => 3,
            Self::Authority { .. } => 4,
        }]);
        field(digest, Some(scope));
        field(digest, Some(key.as_bytes()));
        match self {
            Self::Auxiliary(_) => unreachable!(),
            Self::Operation {
                identity,
                status,
                normal,
                record,
                ..
            } => {
                field(digest, Some(identity));
                digest.update(status.to_be_bytes());
                digest.update([u8::from(*normal)]);
                field(digest, record.as_deref());
            }
            Self::Authority { high, records, .. } => {
                field(digest, Some(high));
                for record in records {
                    field(digest, record.as_deref());
                }
            }
        }
    }
}

impl Ledger {
    /// Each query uses the existing primary-key range, with bounded rows and
    /// bytes. Any admitted mutation invalidates the continuation. Final apply
    /// must independently revalidate the complete selection under writer hold.
    pub(crate) async fn retirement_inventory(&self, cursor: Option<Cursor>) -> Result<Page> {
        self.verify()?;
        let config = self.config.clone();
        self.storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                let context = Context::read(&tx, &config)?;
                if cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.context != context)
                {
                    return Err(invalid());
                }
                let (kind, after, mut audit) = cursor.map_or_else(
                    || {
                        (
                            Kind::Operations,
                            (Vec::new(), String::new()),
                            accounting::Audit::default(),
                        )
                    },
                    |cursor| (cursor.kind, cursor.after, cursor.audit),
                );
                let (rows, more) = page(&tx, &config, kind, &after, BYTES - context.bytes())?;
                for row in &rows {
                    audit.observe(&tx, row)?;
                }
                if !more {
                    audit.end_table(&tx, kind)?;
                    if kind == Kind::Processes {
                        audit.finish(&context)?;
                    }
                }
                let next = if more {
                    let (scope, key) = rows.last().ok_or_else(invalid)?.key();
                    Some(Cursor {
                        context: context.clone(),
                        audit,
                        kind,
                        after: (scope.to_vec(), key.into()),
                    })
                } else {
                    kind.next().map(|kind| Cursor {
                        context: context.clone(),
                        audit,
                        kind,
                        after: (Vec::new(), String::new()),
                    })
                };
                tx.commit().map_err(|_| storage_error())?;
                Ok(Page {
                    context,
                    rows,
                    next,
                })
            })
            .await
    }
}

fn field(digest: &mut Sha256, bytes: Option<&[u8]>) {
    digest.update([u8::from(bytes.is_some())]);
    if let Some(bytes) = bytes {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
}

fn page(
    db: &Connection,
    config: &RuntimeConfig,
    kind: Kind,
    after: &(Vec<u8>, String),
    limit: usize,
) -> Result<(Vec<SourceRow>, bool)> {
    let mut statement = db
        .prepare(match kind {
            Kind::Operations => OPERATIONS,
            Kind::Authority => AUTHORITY,
            Kind::Cleanup => auxiliary::CLEANUP,
            Kind::Scopes => auxiliary::SCOPES,
            Kind::Processes => auxiliary::PROCESSES,
        })
        .map_err(|_| storage_error())?;
    // An integer sorts before every BLOB. Starting below the BLOB domain
    // includes even a damaged empty scope, which validation must refuse.
    let lower = if after.0.is_empty() {
        rusqlite::types::Value::Integer(-1)
    } else {
        rusqlite::types::Value::Blob(after.0.clone())
    };
    let mut query = match kind {
        Kind::Scopes => statement.query(params![lower]),
        _ => statement.query(params![lower, after.1]),
    }
    .map_err(|_| storage_error())?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    while let Some(row) = query.next().map_err(|_| storage_error())? {
        if rows.len() == ROWS {
            return Ok((rows, true));
        }
        // Check SQLite's byte lengths before copying raw payloads into Rust.
        // A damaged row must not defeat the page budget or disappear from it.
        let source_bytes: i64 = row.get(6).map_err(|_| invalid())?;
        if !(0..=BYTES as i64).contains(&source_bytes) {
            return Err(invalid());
        }
        let source = match kind {
            Kind::Cleanup | Kind::Scopes | Kind::Processes => {
                SourceRow::Auxiliary(Auxiliary::read(row, kind)?)
            }
            Kind::Operations => SourceRow::Operation {
                scope: row.get(0).map_err(|_| invalid())?,
                id: row.get(1).map_err(|_| invalid())?,
                identity: row.get(2).map_err(|_| invalid())?,
                status: row.get(3).map_err(|_| invalid())?,
                normal: match row.get::<_, i64>(4).map_err(|_| invalid())? {
                    0 => false,
                    1 => true,
                    _ => return Err(invalid()),
                },
                record: row.get(5).map_err(|_| invalid())?,
            },
            Kind::Authority => SourceRow::Authority {
                scope: row.get(0).map_err(|_| invalid())?,
                resource: row.get(1).map_err(|_| invalid())?,
                high: row.get(2).map_err(|_| invalid())?,
                records: [
                    row.get(3).map_err(|_| invalid())?,
                    row.get(4).map_err(|_| invalid())?,
                    row.get(5).map_err(|_| invalid())?,
                ],
            },
        };
        if source.bytes() > limit {
            return Err(invalid());
        }
        if bytes + source.bytes() > limit {
            return Ok((rows, true));
        }
        validate(config, &source)?;
        bytes += source.bytes();
        rows.push(source);
    }
    Ok((rows, false))
}

fn validate(config: &RuntimeConfig, row: &SourceRow) -> Result<()> {
    let scope: [u8; 8] = row.key().0.try_into().map_err(|_| invalid())?;
    let generation = u64::from_be_bytes(scope);
    if generation > config.generation {
        return Err(invalid());
    }
    let mut scoped = config.clone();
    if generation != 0 {
        scoped.generation = generation;
    }
    match row {
        SourceRow::Auxiliary(row) => row.validate(&scoped, generation)?,
        SourceRow::Operation {
            id,
            identity,
            status,
            record,
            ..
        } => {
            if identity.len() > 65536 {
                return Err(invalid());
            }
            let operation: Operation = serde_json::from_slice(identity).map_err(|_| invalid())?;
            crate::mutation_journal::validate_operation(&scoped, &operation)?;
            if operation.operation_id != *id || journal::scope(&scoped, &operation) != scope {
                return Err(invalid());
            }
            match record {
                Some(bytes) => {
                    let record = Record::decode(bytes, &scoped)?;
                    record.exact(&operation)?;
                    let expected = match record.status() {
                        Status::Started => 1,
                        Status::Completed => 2,
                        Status::Failed => 3,
                        Status::Indeterminate => 4,
                    };
                    if *status != expected {
                        return Err(invalid());
                    }
                }
                None if *status == 0 => {}
                None => return Err(invalid()),
            }
        }
        SourceRow::Authority {
            resource,
            high,
            records,
            ..
        } => {
            let key: ResourceKey = serde_json::from_str(resource).map_err(|_| invalid())?;
            if authority::key(&key)? != *resource
                || authority::decode(
                    &scoped,
                    &key,
                    &scope,
                    high,
                    records.each_ref().map(|r| r.as_deref()),
                )?
                .is_empty()
            {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "retirement_accounting_tests.rs"]
mod accounting_tests;
#[cfg(test)]
#[path = "retirement_context_tests.rs"]
mod context_tests;
#[cfg(test)]
#[path = "retirement_inventory_tests.rs"]
mod tests;
