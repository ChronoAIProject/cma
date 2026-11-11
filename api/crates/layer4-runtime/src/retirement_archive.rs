//! Exact retained bytes and indexed permanent replay refusal. Active-ledger
//! deactivation is a separate, ordered step after this archive has committed.
mod probe;
mod probe_sources;
mod transfer;
pub(crate) use transfer::Direction;
mod indexed;
mod indexed_deactivation;
pub(crate) use indexed_deactivation::deactivate_held;
mod record;
mod schema;
#[cfg(test)]
mod tests;
mod verification;
pub(crate) use probe::run as probe;

pub(crate) use record::{group_key, SourceRecord};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::ledger_storage::Storage;
use crate::mutation_journal::Operation;

pub(crate) struct Archive {
    storage: Storage,
    config: Arc<RuntimeConfig>,
    path: PathBuf,
    identity: [(u64, u64); 3],
    anchor: Option<Arc<std::fs::File>>,
    version: Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Version {
    Legacy = 1,
    Indexed = 2,
}

/// Only a complete source selection and authoritative settled-deletion proof
/// may construct this input. A source digest alone does not grant retirement.
#[derive(Clone)]
pub(crate) struct GroupProof {
    pub(crate) key: String,
    pub(crate) source_sha256: String,
    pub(crate) observation_sha256: String,
    pub(crate) records: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub(crate) records: u64,
    pub(crate) bytes: u64,
    pub(crate) complete: bool,
}

impl Archive {
    pub(crate) async fn open(
        path: PathBuf,
        config: RuntimeConfig,
        archive_id: String,
        create: bool,
        pages: u32,
    ) -> Result<Self> {
        Self::open_version(path, config, archive_id, create, pages, Version::Legacy).await
    }

    pub(crate) async fn open_version(
        path: PathBuf,
        config: RuntimeConfig,
        archive_id: String,
        create: bool,
        pages: u32,
        version: Version,
    ) -> Result<Self> {
        let storage = Storage::open(path.clone(), create, pages).await?;
        let expected = config.clone();
        let storage = storage
            .validate(move |db| schema::open(db, &expected, &archive_id, create, version))
            .await?;
        let identity = match identity(&path) {
            Ok(identity) => identity,
            Err(error) => {
                storage.close().await?;
                return Err(error);
            }
        };
        Ok(Self {
            storage,
            config: Arc::new(config),
            path,
            identity,
            anchor: None,
            version,
        })
    }

    pub(crate) async fn create_anchored(
        directory: Arc<std::fs::File>,
        config: RuntimeConfig,
        archive_id: String,
        pages: u32,
    ) -> Result<Self> {
        let storage = Storage::open_anchored(Arc::clone(&directory), pages).await?;
        let expected = config.clone();
        let storage = storage
            .validate(move |db| schema::open(db, &expected, &archive_id, true, Version::Legacy))
            .await?;
        Ok(Self {
            storage,
            config: Arc::new(config),
            path: PathBuf::new(),
            identity: [(0, 0); 3],
            anchor: Some(directory),
            version: Version::Legacy,
        })
    }

    pub(crate) fn verify(&self) -> Result<()> {
        // An anchored artifact stays confined to its original owned directory
        // even if another actor replaces its visible pathname. The publisher
        // separately verifies that pathname before it can report a ready copy.
        if self.anchor.is_none() && identity(&self.path)? != self.identity {
            return Err(invalid());
        }
        Ok(())
    }

    /// Serving cannot recover active sources while a deactivation is pending.
    /// The bounded partial index makes this independent of archive history size.
    pub(crate) async fn require_ready(&self) -> Result<()> {
        self.verify()?;
        require_reserve(&self.path, self.anchor.as_deref())?;
        self.storage
            .call(|db| {
                let pending: bool = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM retired_groups WHERE complete=0)",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| storage_error())?;
                if pending {
                    return Err(invalid());
                }
                Ok(())
            })
            .await
    }

    /// Check the entire prepared pair before its first active-ledger write.
    /// Both pending deactivation and completed groups permanently refuse.
    pub(crate) async fn authorize(&self, operations: &[Operation]) -> Result<()> {
        self.verify()?;
        require_reserve(&self.path, self.anchor.as_deref())?;
        if operations.is_empty() || operations.len() > 2 {
            return Err(invalid());
        }
        let mut identities = Vec::with_capacity(operations.len());
        for operation in operations {
            crate::mutation_journal::validate_operation(&self.config, operation)
                .map_err(|_| invalid())?;
            identities.push((
                group_key(operation),
                crate::mutation_scope::generation(&self.config, &operation.subject).unwrap_or(0),
                operation.operation_id.clone(),
            ));
        }
        self.storage.call(move |db| {
            for (group, scope, operation) in identities {
                let retired: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM retired_groups WHERE group_key=?) OR EXISTS(SELECT 1 FROM retired_operations WHERE scope=? AND operation_id=?)",
                    params![group, scope.to_be_bytes().as_slice(), operation],
                    |row| row.get(0),
                ).map_err(|_| storage_error())?;
                if retired {
                    return Err(RuntimeError::rejected(
                        "mutation_resource_retired",
                        "runtime mutation resource or operation has been permanently retired",
                    ));
                }
            }
            Ok(())
        }).await
    }

    /// Copy one group a record at a time within one archive transaction. The
    /// caller keeps its writer exclusion throughout selection and deactivation.
    /// A cancelled reply never authorizes deleting source bytes on assumption.
    pub(crate) async fn prepare(
        &self,
        proof: GroupProof,
        mut read: impl FnMut(u32) -> Result<SourceRecord> + Send + 'static,
    ) -> Result<Receipt> {
        self.verify()?;
        if !record::digest(&proof.key)
            || !record::digest(&proof.source_sha256)
            || !record::digest(&proof.observation_sha256)
            || proof.records == 0
            || proof.records > 65_536
        {
            return Err(invalid());
        }
        let config = Arc::clone(&self.config);
        let directory = self.path.clone();
        let anchor = self.anchor.clone();
        self.storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                if let Some(receipt) = existing(&tx, &proof)? {
                    verification::group(&tx, &config, &proof, &receipt)?;
                    return Ok(receipt);
                }
                require_reserve(&directory, anchor.as_deref())?;
                tx.execute(
                    "INSERT INTO retired_groups VALUES (?,?,?,?,1,0)",
                    params![
                        proof.key,
                        proof.source_sha256,
                        proof.observation_sha256,
                        proof.records
                    ],
                )
                .map_err(|_| storage_error())?;
                let mut source_digest = Sha256::new();
                let mut bytes = 0_u64;
                let mut previous = None;
                for index in 0..proof.records {
                    let record = read(index)?;
                    let ordering = (record.scope, record.kind, record.name.clone());
                    if previous.as_ref().is_some_and(|last| last >= &ordering) {
                        return Err(invalid());
                    }
                    previous = Some(ordering);
                    let operation = record.operation(&config)?;
                    if group_key(&operation) != proof.key {
                        return Err(invalid());
                    }
                    record.hash_into(&mut source_digest);
                    bytes = bytes
                        .checked_add(record.bytes.len() as u64)
                        .ok_or_else(invalid)?;
                    let payload_hash = format!("{:x}", Sha256::digest(&record.bytes));
                    tx.execute(
                        "INSERT INTO archived_records VALUES (?,?,?,?,?,?)",
                        params![
                            proof.key,
                            record.scope.to_be_bytes().as_slice(),
                            record.kind,
                            record.name,
                            payload_hash,
                            record.bytes
                        ],
                    )
                    .map_err(|_| storage_error())?;
                    tx.execute(
                        "INSERT INTO retired_operations VALUES (?,?,?) ON CONFLICT DO NOTHING",
                        params![
                            record.scope.to_be_bytes().as_slice(),
                            operation.operation_id,
                            proof.key
                        ],
                    )
                    .map_err(|_| storage_error())?;
                    let owner: String = tx.query_row(
                    "SELECT group_key FROM retired_operations WHERE scope=? AND operation_id=?",
                    params![record.scope.to_be_bytes().as_slice(), operation.operation_id],
                    |row| row.get(0),
                ).map_err(|_| storage_error())?;
                    if owner != proof.key {
                        return Err(invalid());
                    }
                }
                if format!("{:x}", source_digest.finalize()) != proof.source_sha256 {
                    return Err(invalid());
                }
                let signed_bytes = i64::try_from(bytes).map_err(|_| invalid())?;
                tx.execute(
                    "UPDATE retired_groups SET bytes=? WHERE group_key=?",
                    params![signed_bytes, proof.key],
                )
                .map_err(|_| storage_error())?;
                tx.commit().map_err(|_| storage_error())?;
                let receipt = existing(db, &proof)?.ok_or_else(invalid)?;
                verification::group(db, &config, &proof, &receipt)?;
                Ok(receipt)
            })
            .await
    }

    /// Fixture-only completion. Production transitions use transfer_group.
    #[cfg(test)]
    pub(crate) async fn complete(&self, key: String, source_sha256: String) -> Result<()> {
        self.verify()?;
        if !record::digest(&key) || !record::digest(&source_sha256) {
            return Err(invalid());
        }
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                let (observation_sha256, records): (String, u32) = tx.query_row(
                    "SELECT observation_sha256,records FROM retired_groups WHERE group_key=? AND source_sha256=?",
                    params![key, source_sha256], |row| Ok((row.get(0)?, row.get(1)?)),
                ).map_err(|_| invalid())?;
                if !record::digest(&observation_sha256) || records == 0 || records > 65_536 {
                    return Err(invalid());
                }
                let proof = GroupProof { key, source_sha256, observation_sha256, records };
                let receipt = existing(&tx, &proof)?.ok_or_else(invalid)?;
                verification::group(&tx, &config, &proof, &receipt)?;
                let changed = tx.execute(
                "UPDATE retired_groups SET complete=1 WHERE group_key=? AND source_sha256=?",
                params![proof.key, proof.source_sha256],
            ).map_err(|_| storage_error())?;
                if changed != 1 {
                    return Err(invalid());
                }
                tx.commit().map_err(|_| storage_error())
            })
            .await
    }

    pub(crate) async fn close(self) -> Result<()> {
        self.storage.close().await
    }
}

fn existing(db: &rusqlite::Connection, proof: &GroupProof) -> Result<Option<Receipt>> {
    type Existing = (String, String, i64, i64, bool);
    let existing: Option<Existing> = db.query_row(
        "SELECT source_sha256,observation_sha256,records,bytes,complete FROM retired_groups WHERE group_key=?",
        [&proof.key],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    ).optional().map_err(|_| storage_error())?;
    let Some((source, observation, records, bytes, complete)) = existing else {
        return Ok(None);
    };
    let records = u64::try_from(records).map_err(|_| invalid())?;
    let bytes = u64::try_from(bytes).map_err(|_| invalid())?;
    if source != proof.source_sha256
        || observation != proof.observation_sha256
        || records != u64::from(proof.records)
        || bytes == 0
    {
        return Err(invalid());
    }
    Ok(Some(Receipt {
        records,
        bytes,
        complete,
    }))
}

fn require_reserve(path: &Path, anchor: Option<&std::fs::File>) -> Result<()> {
    match anchor {
        Some(anchor) => crate::ledger_storage::anchored::require(anchor),
        None => crate::ledger_storage::reserve::require(path),
    }
}

fn identity(path: &Path) -> Result<[(u64, u64); 3]> {
    use std::os::unix::fs::MetadataExt;
    for ancestor in path.ancestors() {
        if !std::fs::symlink_metadata(ancestor)
            .map_err(|_| invalid())?
            .is_dir()
        {
            return Err(invalid());
        }
    }
    let paths = [
        path.to_owned(),
        path.join("ledger.sqlite3"),
        path.join("writer.lock"),
    ];
    let mut result = [(0, 0); 3];
    for (index, path) in paths.iter().enumerate() {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| invalid())?;
        if (index == 0 && !metadata.is_dir())
            || (index != 0 && (!metadata.is_file() || metadata.nlink() != 1))
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid());
        }
        result[index] = (metadata.dev(), metadata.ino());
    }
    Ok(result)
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_retirement_archive_invalid",
        "runtime retirement archive requires verification",
    )
}

fn storage_error() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_storage_hold",
        "runtime mutation ledger storage requires verification",
    )
}
