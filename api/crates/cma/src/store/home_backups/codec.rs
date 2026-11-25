//! Strict versioned DTOs preserve the catalog's provider-facing BSON names.
use super::SCHEMA_VERSION;
use layer2_core::sandbox::home_backup::*;
use mongodb::bson::{deserialize_from_document, serialize_to_document, Document};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLease {
    holder: String,
    token: String,
    fence: i64,
    expires_at: String,
}
impl StoredLease {
    fn encode(value: &HomeBackupLease) -> Result<Self, layer2_core::StoreError> {
        Ok(Self {
            holder: value.holder.clone(),
            token: value.token.clone(),
            fence: signed(value.fence)?,
            expires_at: value.expires_at.clone(),
        })
    }
    fn decode(self) -> Result<HomeBackupLease, layer2_core::StoreError> {
        Ok(HomeBackupLease {
            holder: self.holder,
            token: self.token,
            fence: u64::try_from(self.fence).map_err(corrupt)?,
            expires_at: self.expires_at,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSchedule {
    id: String,
    home: StoredHome,
    state: HomeBackupScheduleState,
    next_due_at: String,
    last_ready_at: Option<String>,
    last_ready_backup_id: Option<String>,
    lease: Option<StoredLease>,
    next_fence: i64,
    in_flight: Option<HomeBackupInFlight>,
    revision: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredHome {
    mode: String,
    pvc_uid: String,
    home_identity: String,
    storage_reference: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredBackup {
    id: String,
    sandbox_id: String,
    epoch: i64,
    source: StoredSource,
    cut: StoredCut,
    storage: StoredStorage,
    custody: StoredCustody,
    retention: HomeBackupRetention,
    state: HomeBackupState,
    restore_pins: i64,
    lease: Option<StoredLease>,
    next_fence: i64,
    revision: i64,
    failure: Option<layer2_core::sandbox::home_backup::HomeBackupFailure>,
    erasure: Option<HomeBackupErasure>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSource {
    pvc_uid: String,
    home_identity: String,
    runtime_generation: i64,
    runtime_version: String,
    boot_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCut {
    requested_at: String,
    provider_creation_time: Option<String>,
    ready_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredStorage {
    snapshot_name: String,
    snapshot_class: String,
    storage_handle: Option<String>,
    restore_size_bytes: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCustody {
    cmek_key_version_id: Option<String>,
    auth_key_ids: Vec<String>,
}

pub(super) fn schedule_document(
    value: &HomeBackupSchedule,
) -> Result<Document, layer2_core::StoreError> {
    value.validate().map_err(corrupt)?;
    let stored = StoredSchedule {
        id: value.id.clone(),
        home: StoredHome {
            mode: value.home.mode.clone(),
            pvc_uid: value.home.home_source_id.clone(),
            home_identity: value.home.home_identity.clone(),
            storage_reference: value.home.source_reference.clone(),
        },
        state: value.state,
        next_due_at: value.next_due_at.clone(),
        last_ready_at: value.last_ready_at.clone(),
        last_ready_backup_id: value.last_ready_backup_id.clone(),
        lease: value.lease.as_ref().map(StoredLease::encode).transpose()?,
        next_fence: signed(value.next_fence)?,
        in_flight: value.in_flight.clone(),
        revision: signed(value.revision)?,
    };
    let mut document = serialize_to_document(&stored).map_err(corrupt)?;
    document.insert("_id", value.id.clone());
    document.insert("document_schema_version", SCHEMA_VERSION);
    Ok(document)
}

pub(super) fn backup_document(value: &HomeBackup) -> Result<Document, layer2_core::StoreError> {
    value.validate().map_err(corrupt)?;
    let stored = StoredBackup {
        id: value.id.clone(),
        sandbox_id: value.sandbox_id.clone(),
        epoch: signed(value.epoch)?,
        source: StoredSource {
            pvc_uid: value.source.home_source_id.clone(),
            home_identity: value.source.home_identity.clone(),
            runtime_generation: signed(value.source.runtime_generation)?,
            runtime_version: value.source.runtime_version.clone(),
            boot_id: value.source.boot_id.clone(),
        },
        cut: StoredCut {
            requested_at: value.cut.requested_at.clone(),
            provider_creation_time: value.cut.source_creation_time.clone(),
            ready_at: value.cut.ready_at.clone(),
        },
        storage: StoredStorage {
            snapshot_name: value.storage.snapshot_name.clone(),
            snapshot_class: value.storage.snapshot_class.clone(),
            storage_handle: value.storage.source_reference.clone(),
            restore_size_bytes: value.storage.restore_size_bytes.map(signed).transpose()?,
        },
        custody: StoredCustody {
            cmek_key_version_id: value.custody.encryption_key_version_id.clone(),
            auth_key_ids: value.custody.auth_key_ids.clone(),
        },
        retention: value.retention.clone(),
        state: value.state,
        restore_pins: i64::from(value.restore_pins),
        lease: value.lease.as_ref().map(StoredLease::encode).transpose()?,
        next_fence: signed(value.next_fence)?,
        revision: signed(value.revision)?,
        failure: value.failure,
        erasure: value.erasure.clone(),
    };
    let mut document = serialize_to_document(&stored).map_err(corrupt)?;
    document.insert("_id", value.id.clone());
    document.insert("document_schema_version", SCHEMA_VERSION);
    Ok(document)
}

pub(super) fn read_schedule(
    mut document: Document,
) -> Result<HomeBackupSchedule, layer2_core::StoreError> {
    if document
        .get_i32("document_schema_version")
        .map_err(corrupt)?
        != SCHEMA_VERSION
    {
        return Err(corrupt("unsupported home backup schedule schema"));
    }
    let original = document.clone();
    let id = document.get_str("_id").map_err(corrupt)?.to_owned();
    document.remove("_id");
    document.remove("document_schema_version");
    let stored: StoredSchedule = deserialize_from_document(document).map_err(corrupt)?;
    let value = HomeBackupSchedule {
        id: stored.id,
        home: HomeBackupHome {
            mode: stored.home.mode,
            home_source_id: stored.home.pvc_uid,
            home_identity: stored.home.home_identity,
            source_reference: stored.home.storage_reference,
        },
        state: stored.state,
        next_due_at: stored.next_due_at,
        last_ready_at: stored.last_ready_at,
        last_ready_backup_id: stored.last_ready_backup_id,
        lease: stored.lease.map(StoredLease::decode).transpose()?,
        next_fence: u64::try_from(stored.next_fence).map_err(corrupt)?,
        in_flight: stored.in_flight,
        revision: u64::try_from(stored.revision).map_err(corrupt)?,
    };
    if value.id != id {
        return Err(corrupt("schedule id disagrees with _id"));
    }
    value.validate().map_err(corrupt)?;
    let expected = schedule_document(&value)?;
    require_exact(&original, &expected)?;
    Ok(value)
}

pub(super) fn read_backup(mut document: Document) -> Result<HomeBackup, layer2_core::StoreError> {
    if document
        .get_i32("document_schema_version")
        .map_err(corrupt)?
        != SCHEMA_VERSION
    {
        return Err(corrupt("unsupported home backup schema"));
    }
    let original = document.clone();
    let id = document.get_str("_id").map_err(corrupt)?.to_owned();
    document.remove("_id");
    document.remove("document_schema_version");
    let stored: StoredBackup = deserialize_from_document(document).map_err(corrupt)?;
    let value = HomeBackup {
        id: stored.id,
        sandbox_id: stored.sandbox_id,
        epoch: u64::try_from(stored.epoch).map_err(corrupt)?,
        source: HomeBackupSource {
            home_source_id: stored.source.pvc_uid,
            home_identity: stored.source.home_identity,
            runtime_generation: u64::try_from(stored.source.runtime_generation).map_err(corrupt)?,
            runtime_version: stored.source.runtime_version,
            boot_id: stored.source.boot_id,
        },
        cut: HomeBackupCut {
            requested_at: stored.cut.requested_at,
            source_creation_time: stored.cut.provider_creation_time,
            ready_at: stored.cut.ready_at,
        },
        storage: HomeBackupStorage {
            snapshot_name: stored.storage.snapshot_name,
            snapshot_class: stored.storage.snapshot_class,
            source_reference: stored.storage.storage_handle,
            restore_size_bytes: stored
                .storage
                .restore_size_bytes
                .map(|v| u64::try_from(v).map_err(corrupt))
                .transpose()?,
        },
        custody: HomeBackupCustody {
            encryption_key_version_id: stored.custody.cmek_key_version_id,
            auth_key_ids: stored.custody.auth_key_ids,
        },
        retention: stored.retention,
        state: stored.state,
        restore_pins: u32::try_from(stored.restore_pins).map_err(corrupt)?,
        lease: stored.lease.map(StoredLease::decode).transpose()?,
        next_fence: u64::try_from(stored.next_fence).map_err(corrupt)?,
        revision: u64::try_from(stored.revision).map_err(corrupt)?,
        failure: stored.failure,
        erasure: stored.erasure,
    };
    if value.id != id {
        return Err(corrupt("backup id disagrees with _id"));
    }
    value.validate().map_err(corrupt)?;
    let expected = backup_document(&value)?;
    require_exact(&original, &expected)?;
    Ok(value)
}

pub(super) fn signed(value: u64) -> Result<i64, layer2_core::StoreError> {
    i64::try_from(value).map_err(|_| corrupt("fence exceeds signed storage range"))
}
pub(super) fn corrupt(error: impl std::fmt::Display) -> layer2_core::StoreError {
    layer2_core::StoreError::Malformed(error.to_string())
}

fn require_exact(actual: &Document, expected: &Document) -> Result<(), layer2_core::StoreError> {
    if actual.len() != expected.len()
        || expected
            .iter()
            .any(|(key, value)| actual.get(key) != Some(value))
    {
        return Err(corrupt("home backup envelope disagrees with strict record"));
    }
    Ok(())
}
