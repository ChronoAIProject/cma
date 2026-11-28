//! Mongo persistence for sandbox aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::{SandboxId, SubjectId};
use layer2_core::sandbox::store::{
    valid_admission_fingerprint, Page, PageRequest, SandboxAdmissionName, SandboxAdmissionReplay,
    SandboxStore, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{Sandbox, SandboxOrigin};
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    i64_field_matches, is_duplicate_key, nullable, record_bson, signed, string_field_matches,
    MongoSandboxStore,
};

pub(super) fn sandbox_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "owner": 1, "name": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_owner_name_live".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "retired_at": Bson::Null })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "origin_launch_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_owner_trigger_launch".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "trigger" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "record.owner": 1, "record.origin.launch_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_record_owner_trigger_launch".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "record.origin.type": "trigger" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_due_work".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "status": 1, "updated_at": -1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_owner_status_updated".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! {
                "owner": 1,
                "status": 1,
                "runtime_version": 1,
                "_id": 1,
            })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_operator_inventory".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_owner_page".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "status": 1, "_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_owner_status_page".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "status": 1, "_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_maintenance_page".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "origin_operation_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_owner_lifecycle_operation".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "lifecycle" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "record.owner": 1, "record.origin.operation_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_record_owner_lifecycle_operation".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "record.origin.type": "lifecycle" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "retired_at": 1, "_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("sandboxes_global_live".to_string())
                    .build(),
            )
            .build(),
    ]
}

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(sandbox_indexes())
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(format!("sandbox indexes: {error}")))
}

pub(super) fn sandbox_document(
    sandbox: &Sandbox,
    last_seq: u64,
) -> Result<Document, SandboxStoreError> {
    if sandbox.origin.replay_key().is_some() {
        return Err(SandboxStoreError::InvalidTransaction(
            "origin-bound sandboxes require atomic admission",
        ));
    }
    raw_sandbox_document(sandbox, last_seq, Bson::Null, Bson::Null)
}

pub(super) fn admitted_sandbox_document(
    sandbox: &Sandbox,
    last_seq: u64,
    admission_name: &SandboxAdmissionName,
    admission_fingerprint: &str,
) -> Result<Document, SandboxStoreError> {
    if !valid_admission_fingerprint(admission_fingerprint) {
        return Err(SandboxStoreError::InvalidTransaction(
            "sandbox admission fingerprint is invalid",
        ));
    }
    let (name, fingerprint) = match &sandbox.origin {
        SandboxOrigin::Manual => (Bson::Null, Bson::Null),
        SandboxOrigin::Trigger { .. } | SandboxOrigin::Lifecycle { .. } => {
            SandboxAdmissionReplay::new(
                sandbox.clone(),
                admission_name.clone(),
                admission_fingerprint.to_string(),
            )?;
            (
                record_bson(admission_name).map_err(SandboxStoreError::Corrupt)?,
                Bson::String(admission_fingerprint.to_string()),
            )
        }
    };
    raw_sandbox_document(sandbox, last_seq, name, fingerprint)
}

fn raw_sandbox_document(
    sandbox: &Sandbox,
    last_seq: u64,
    admission_name: Bson,
    admission_fingerprint: Bson,
) -> Result<Document, SandboxStoreError> {
    sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("sandbox aggregate is invalid: {error}"))
    })?;
    let (origin_type, origin_launch_id, origin_operation_id) = sandbox_origin_fields(sandbox)?;
    let lease_expires_at = sandbox.work.lease.as_ref().map(|lease| &lease.expires_at);
    Ok(doc! {
        "_id": sandbox.id.as_str(),
        "document_schema_version": super::work_schema::sandbox_version(sandbox),
        "owner": sandbox.owner.as_str(),
        "name": &sandbox.name,
        "origin_type": origin_type,
        "origin_launch_id": origin_launch_id,
        "origin_operation_id": origin_operation_id,
        "admission_name": admission_name,
        "admission_fingerprint": admission_fingerprint,
        "status": record_bson(&sandbox.status).map_err(SandboxStoreError::Corrupt)?,
        "runtime_version": nullable(
            sandbox.runtime.as_ref().map(|runtime| &runtime.runtime_version)
        ),
        "updated_at": &sandbox.updated_at,
        "retired_at": nullable(sandbox.retired_at.as_ref()),
        "purge_after": nullable(sandbox.purge_after.as_ref()),
        "work_pending": sandbox.work.step.is_pending(),
        "work_ready_at": &sandbox.work.ready_at,
        "work_lease_expires_at": nullable(lease_expires_at),
        "revision": signed(sandbox.revision, "sandbox revision")
            .map_err(SandboxStoreError::Corrupt)?,
        "last_seq": signed(last_seq, "sandbox event sequence")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(sandbox).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_sandbox(document: &Document) -> Result<Sandbox, SandboxStoreError> {
    if super::sandbox_quarantine::contains(document) {
        return Err(SandboxStoreError::Corrupt(
            "retained Sandbox is quarantined".into(),
        ));
    }
    let sandbox: Sandbox =
        super::work_schema::read(document).map_err(SandboxStoreError::Corrupt)?;
    sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("stored sandbox is invalid: {error}"))
    })?;
    let revision =
        signed(sandbox.revision, "sandbox revision").map_err(SandboxStoreError::Corrupt)?;
    let status = record_bson(&sandbox.status).map_err(SandboxStoreError::Corrupt)?;
    let runtime_version = nullable(
        sandbox
            .runtime
            .as_ref()
            .map(|runtime| &runtime.runtime_version),
    );
    let (origin_type, origin_launch_id, origin_operation_id) = sandbox_origin_fields(&sandbox)?;
    if !string_field_matches(document, "_id", sandbox.id.as_str())
        || !string_field_matches(document, "owner", sandbox.owner.as_str())
        || !string_field_matches(document, "name", &sandbox.name)
        || !string_field_matches(document, "origin_type", origin_type)
        || document.get("origin_launch_id") != Some(&origin_launch_id)
        || document.get("origin_operation_id").unwrap_or(&Bson::Null) != &origin_operation_id
        || document.get("status") != Some(&status)
        || document.get("runtime_version") != Some(&runtime_version)
        || !string_field_matches(document, "updated_at", &sandbox.updated_at)
        || !i64_field_matches(document, "revision", revision)
        || document.get("retired_at") != Some(&nullable(sandbox.retired_at.as_ref()))
        || document.get("purge_after") != Some(&nullable(sandbox.purge_after.as_ref()))
        || document.get_bool("work_pending").ok() != Some(sandbox.work.step.is_pending())
        || !string_field_matches(document, "work_ready_at", &sandbox.work.ready_at)
        || document.get("work_lease_expires_at")
            != Some(&nullable(
                sandbox.work.lease.as_ref().map(|lease| &lease.expires_at),
            ))
    {
        return Err(SandboxStoreError::Corrupt(
            "sandbox query fields disagree with its aggregate".into(),
        ));
    }
    read_admission_replay_metadata(document, &sandbox)?;
    crate::store::legacy_finalization::validate_document(document, &sandbox).map_err(|_| {
        SandboxStoreError::Corrupt("Channel retirement finalization is inconsistent".into())
    })?;
    Ok(sandbox)
}

pub(super) fn read_sandbox_admission_replay(
    document: &Document,
) -> Result<SandboxAdmissionReplay, SandboxStoreError> {
    let sandbox = read_sandbox(document)?;
    read_admission_replay_metadata(document, &sandbox)?.ok_or_else(|| {
        SandboxStoreError::Corrupt("manual sandbox cannot be an origin replay".into())
    })
}

pub(super) fn sandbox_origin_fields(
    sandbox: &Sandbox,
) -> Result<(&'static str, Bson, Bson), SandboxStoreError> {
    match &sandbox.origin {
        SandboxOrigin::Manual => Ok(("manual", Bson::Null, Bson::Null)),
        SandboxOrigin::Trigger { launch_id }
            if !launch_id.trim().is_empty()
                && launch_id.len() <= 128
                && !launch_id.chars().any(char::is_control) =>
        {
            Ok(("trigger", Bson::String(launch_id.clone()), Bson::Null))
        }
        SandboxOrigin::Lifecycle { operation_id }
            if !operation_id.trim().is_empty()
                && operation_id.len() <= 128
                && !operation_id.chars().any(char::is_control) =>
        {
            Ok(("lifecycle", Bson::Null, Bson::String(operation_id.clone())))
        }
        SandboxOrigin::Trigger { .. } | SandboxOrigin::Lifecycle { .. } => Err(
            SandboxStoreError::Corrupt("stored sandbox admission origin is invalid".into()),
        ),
    }
}

fn read_admission_replay_metadata(
    document: &Document,
    sandbox: &Sandbox,
) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError> {
    match (
        &sandbox.origin,
        document.get("admission_name"),
        document.get("admission_fingerprint"),
    ) {
        (SandboxOrigin::Manual, Some(Bson::Null), Some(Bson::Null)) => Ok(None),
        (
            SandboxOrigin::Trigger { .. } | SandboxOrigin::Lifecycle { .. },
            Some(name),
            Some(Bson::String(fingerprint)),
        ) => {
            let decoded =
                mongodb::bson::deserialize_from_bson::<SandboxAdmissionName>(name.clone());
            let name = decoded.map_err(|error| {
                SandboxStoreError::Corrupt(format!("sandbox admission name is invalid: {error}"))
            })?;
            SandboxAdmissionReplay::from_retained(sandbox.clone(), name, fingerprint.clone())
                .map(Some)
        }
        _ => Err(SandboxStoreError::Corrupt(
            "sandbox admission metadata disagrees with its origin".into(),
        )),
    }
}

pub(super) fn sandbox_replacement_fields(sandbox: &Sandbox) -> Result<Document, SandboxStoreError> {
    if sandbox.retirement_archive_pending() {
        return Err(SandboxStoreError::Conflict);
    }
    let mut fields = raw_sandbox_document(sandbox, 0, Bson::Null, Bson::Null)?;
    fields.remove("_id");
    fields.remove("last_seq");
    fields.remove("admission_name");
    fields.remove("admission_fingerprint");
    Ok(fields)
}

pub(super) mod capacity;
mod failed_admission;
mod implementation;
