//! BSON records and indexes for Worktree aggregates.

use cma_am_mongo_primitives::{
    i64_field_matches, read_record, record_bson, signed, string_field_matches,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::Worktree;
use mongodb::bson::{doc, Document};
use mongodb::{options::IndexOptions, IndexModel};

pub const DOCUMENT_SCHEMA_VERSION: i64 = 1;

pub fn worktree_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "workspace_id": 1, "name": 1 })
            .options(
                IndexOptions::builder()
                    .name("worktrees_workspace_name".to_string())
                    .unique(true)
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("worktrees_sandbox".to_string())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "record.sandbox_id": 1 })
            .build(),
    ]
}

pub fn worktree_document(worktree: &Worktree) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": worktree.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "workspace_id": worktree.workspace_id.as_str(),
        "sandbox_id": worktree.sandbox_id.as_str(),
        "owner": worktree.owner.as_str(),
        "name": &worktree.name,
        "status": record_bson(&worktree.status).map_err(SandboxStoreError::Corrupt)?,
        "revision": signed(worktree.revision, "worktree revision")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(worktree).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub fn read_worktree(document: &Document) -> Result<Worktree, SandboxStoreError> {
    let worktree: Worktree =
        read_record(document, DOCUMENT_SCHEMA_VERSION).map_err(SandboxStoreError::Corrupt)?;
    let status = record_bson(&worktree.status).map_err(SandboxStoreError::Corrupt)?;
    let revision =
        signed(worktree.revision, "worktree revision").map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", worktree.id.as_str())
        || !string_field_matches(document, "workspace_id", worktree.workspace_id.as_str())
        || !string_field_matches(document, "sandbox_id", worktree.sandbox_id.as_str())
        || !string_field_matches(document, "owner", worktree.owner.as_str())
        || !string_field_matches(document, "name", &worktree.name)
        || document.get("status") != Some(&status)
        || !i64_field_matches(document, "revision", revision)
    {
        return Err(SandboxStoreError::Corrupt(
            "worktree query fields disagree with its aggregate".into(),
        ));
    }
    Ok(worktree)
}

pub fn worktree_replacement_fields(worktree: &Worktree) -> Result<Document, SandboxStoreError> {
    let mut document = worktree_document(worktree)?;
    document.remove("_id");
    Ok(document)
}
