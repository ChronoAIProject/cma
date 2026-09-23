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
    i64_field_matches, is_duplicate_key, nullable, read_record, record_bson, signed,
    string_field_matches, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
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
    if matches!(&sandbox.origin, SandboxOrigin::Trigger { .. }) {
        return Err(SandboxStoreError::InvalidTransaction(
            "trigger sandboxes require atomic admission",
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
        SandboxOrigin::Trigger { .. } => {
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
    let (origin_type, origin_launch_id) = sandbox_origin_fields(sandbox)?;
    let lease_expires_at = sandbox.work.lease.as_ref().map(|lease| &lease.expires_at);
    Ok(doc! {
        "_id": sandbox.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": sandbox.owner.as_str(),
        "name": &sandbox.name,
        "origin_type": origin_type,
        "origin_launch_id": origin_launch_id,
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
    let sandbox: Sandbox = read_record(document).map_err(SandboxStoreError::Corrupt)?;
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
    let (origin_type, origin_launch_id) = sandbox_origin_fields(&sandbox)?;
    if !string_field_matches(document, "_id", sandbox.id.as_str())
        || !string_field_matches(document, "owner", sandbox.owner.as_str())
        || !string_field_matches(document, "name", &sandbox.name)
        || !string_field_matches(document, "origin_type", origin_type)
        || document.get("origin_launch_id") != Some(&origin_launch_id)
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
) -> Result<(&'static str, Bson), SandboxStoreError> {
    match &sandbox.origin {
        SandboxOrigin::Manual => Ok(("manual", Bson::Null)),
        SandboxOrigin::Trigger { launch_id }
            if !launch_id.trim().is_empty()
                && launch_id.len() <= 128
                && !launch_id.chars().any(char::is_control) =>
        {
            Ok(("trigger", Bson::String(launch_id.clone())))
        }
        SandboxOrigin::Trigger { .. } => Err(SandboxStoreError::Corrupt(
            "stored sandbox trigger origin is invalid".into(),
        )),
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
        (SandboxOrigin::Trigger { .. }, Some(name), Some(Bson::String(fingerprint))) => {
            let decoded =
                mongodb::bson::deserialize_from_bson::<SandboxAdmissionName>(name.clone());
            let name = decoded.map_err(|error| {
                SandboxStoreError::Corrupt(format!("sandbox admission name is invalid: {error}"))
            })?;
            SandboxAdmissionReplay::new(sandbox.clone(), name, fingerprint.clone()).map(Some)
        }
        _ => Err(SandboxStoreError::Corrupt(
            "sandbox admission metadata disagrees with its origin".into(),
        )),
    }
}

pub(super) fn sandbox_replacement_fields(sandbox: &Sandbox) -> Result<Document, SandboxStoreError> {
    let mut fields = raw_sandbox_document(sandbox, 0, Bson::Null, Bson::Null)?;
    fields.remove("_id");
    fields.remove("last_seq");
    fields.remove("admission_name");
    fields.remove("admission_fingerprint");
    Ok(fields)
}

async fn classify_cas_miss(
    collection: &Collection<Document>,
    id: &SandboxId,
) -> Result<SandboxStoreError, SandboxStoreError> {
    match collection
        .find_one(doc! { "_id": id.as_str() })
        .projection(doc! { "document_schema_version": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        None => Ok(SandboxStoreError::NotFound),
        Some(document) => {
            super::sandbox_mongo::require_version(&document).map_err(SandboxStoreError::Corrupt)?;
            Ok(SandboxStoreError::Conflict)
        }
    }
}

#[async_trait::async_trait]
impl SandboxStore for MongoSandboxStore {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        if !sandbox.credential.service_grants.is_empty() {
            return Err(SandboxStoreError::InvalidTransaction(
                "service access requires an existing sandbox and an atomic grant",
            ));
        }
        if sandbox.revision != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        let document = sandbox_document(&sandbox, 0)?;
        self.sandboxes
            .insert_one(document)
            .await
            .map(|_| ())
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    SandboxStoreError::Duplicate
                } else {
                    SandboxStoreError::Unavailable(error.to_string())
                }
            })
    }

    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        self.sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_sandbox)
            .transpose()
    }

    async fn save_sandbox(
        &self,
        expected_revision: u64,
        sandbox: Sandbox,
    ) -> Result<(), SandboxStoreError> {
        let next = expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        if sandbox.revision != next {
            return Err(SandboxStoreError::Conflict);
        }
        let current = self
            .get_sandbox(&sandbox.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.revision != expected_revision {
            return Err(SandboxStoreError::Conflict);
        }
        current
            .validate_service_access_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        current
            .validate_checkpoint_capture_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        layer2_core::sandbox::ProvisioningExecution::validate_successor(
            current.work.execution,
            sandbox.work.execution,
        )
        .map_err(|_| SandboxStoreError::Conflict)?;
        current
            .validate_runtime_generation_successor(&sandbox)
            .map_err(|_| SandboxStoreError::Conflict)?;
        let expected = signed(expected_revision, "expected sandbox revision")
            .map_err(|_| SandboxStoreError::Conflict)?;
        let (origin_type, origin_launch_id) = sandbox_origin_fields(&sandbox)?;
        let result = self
            .sandboxes
            .update_one(
                doc! {
                    "_id": sandbox.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": expected,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                    "channel_retirement_finalization": { "$exists": false },
                    "origin_type": origin_type,
                    "origin_launch_id": origin_launch_id,
                },
                doc! { "$set": sandbox_replacement_fields(&sandbox)? },
            )
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    SandboxStoreError::Duplicate
                } else {
                    SandboxStoreError::Unavailable(error.to_string())
                }
            })?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(classify_cas_miss(&self.sandboxes, &sandbox.id).await?)
        }
    }

    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "owner": owner.as_str() };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandboxes
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            items.push(read_sandbox(&document)?);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded page has a last item")
                .id
                .0
                .clone()
        });
        Ok(Page { items, next })
    }
}
