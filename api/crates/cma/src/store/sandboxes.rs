//! Mongo persistence for sandbox aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::{SandboxId, SubjectId};
use layer2_core::sandbox::store::{
    Page, PageRequest, SandboxAdmissionReplay, SandboxStore, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Sandbox;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{is_duplicate_key, MongoSandboxStore};

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

pub(super) use cma_am_sandbox_state::sandbox::{
    admitted_sandbox_document, sandbox_document, sandbox_origin_fields, sandbox_replacement_fields,
};

pub(super) fn read_sandbox(document: &Document) -> Result<Sandbox, SandboxStoreError> {
    if super::sandbox_quarantine::contains(document) {
        return Err(SandboxStoreError::Corrupt(
            "retained Sandbox is quarantined".into(),
        ));
    }
    let sandbox = cma_am_sandbox_state::sandbox::read_sandbox(document)?;
    crate::store::legacy_finalization::validate_document(document, &sandbox).map_err(|_| {
        SandboxStoreError::Corrupt("Channel retirement finalization is inconsistent".into())
    })?;
    Ok(sandbox)
}

pub(super) fn read_sandbox_admission_replay(
    document: &Document,
) -> Result<SandboxAdmissionReplay, SandboxStoreError> {
    let sandbox = read_sandbox(document)?;
    cma_am_sandbox_state::sandbox::read_admission_replay_metadata(document, &sandbox)?.ok_or_else(
        || SandboxStoreError::Corrupt("manual sandbox cannot be an origin replay".into()),
    )
}

pub(super) mod capacity;
mod failed_admission;
mod implementation;
