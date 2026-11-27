//! Mongo persistence for worktree aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::{WorkspaceId, WorktreeId};
use layer2_core::sandbox::store::{
    Page, PageRequest, SandboxStoreError, WorktreeStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Worktree;
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    i64_field_matches, is_duplicate_key, read_record, record_bson, require_version, signed,
    string_field_matches, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) fn worktree_indexes() -> Vec<IndexModel> {
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

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(worktree_indexes())
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(format!("worktree indexes: {error}")))
}

pub(super) fn worktree_document(worktree: &Worktree) -> Result<Document, SandboxStoreError> {
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

pub(super) fn read_worktree(document: &Document) -> Result<Worktree, SandboxStoreError> {
    let worktree: Worktree = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    validate_worktree(document, worktree)
}

pub(super) fn read_worktree_owned(mut document: Document) -> Result<Worktree, SandboxStoreError> {
    require_version(&document).map_err(SandboxStoreError::Corrupt)?;
    let record = document.remove("record").ok_or_else(|| {
        SandboxStoreError::Corrupt("stored document has no aggregate record".into())
    })?;
    let worktree = mongodb::bson::deserialize_from_bson(record)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    validate_worktree(&document, worktree)
}

fn validate_worktree(
    document: &Document,
    worktree: Worktree,
) -> Result<Worktree, SandboxStoreError> {
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

pub(super) fn worktree_replacement_fields(
    worktree: &Worktree,
) -> Result<Document, SandboxStoreError> {
    let mut document = worktree_document(worktree)?;
    document.remove("_id");
    Ok(document)
}

async fn classify_miss(
    collection: &Collection<Document>,
    id: &WorktreeId,
) -> Result<SandboxStoreError, SandboxStoreError> {
    match collection
        .find_one(doc! { "_id": id.as_str() })
        .projection(doc! { "document_schema_version": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        None => Ok(SandboxStoreError::NotFound),
        Some(document) => {
            require_version(&document).map_err(SandboxStoreError::Corrupt)?;
            Ok(SandboxStoreError::Conflict)
        }
    }
}

#[async_trait::async_trait]
impl WorktreeStore for MongoSandboxStore {
    async fn create_worktree(&self, worktree: Worktree) -> Result<(), SandboxStoreError> {
        if worktree.revision != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        self.worktrees
            .insert_one(worktree_document(&worktree)?)
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

    async fn get_worktree(&self, id: &WorktreeId) -> Result<Option<Worktree>, SandboxStoreError> {
        self.worktrees
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_worktree)
            .transpose()
    }

    async fn save_worktree(
        &self,
        expected_revision: u64,
        worktree: Worktree,
    ) -> Result<(), SandboxStoreError> {
        if worktree.revision
            != expected_revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?
        {
            return Err(SandboxStoreError::Conflict);
        }
        let result = self
            .worktrees
            .update_one(
                doc! {
                    "_id": worktree.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": signed(expected_revision, "expected worktree revision")
                        .map_err(|_| SandboxStoreError::Conflict)?,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                },
                doc! { "$set": worktree_replacement_fields(&worktree)? },
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
            Err(classify_miss(&self.worktrees, &worktree.id).await?)
        }
    }

    async fn list_worktrees(
        &self,
        workspace_id: &WorkspaceId,
        page: &PageRequest,
    ) -> Result<Page<Worktree>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "workspace_id": workspace_id.as_str() };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .worktrees
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
            items.push(read_worktree(&document)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_name_index_is_unconditionally_unique() {
        let indexes = worktree_indexes();
        let index = &indexes[0];
        assert_eq!(index.keys, doc! { "workspace_id": 1, "name": 1 });
        assert_eq!(index.options.as_ref().unwrap().unique, Some(true));
        assert_eq!(indexes[1].keys, doc! { "sandbox_id": 1 });
        assert_eq!(
            indexes[1].options.as_ref().unwrap().name.as_deref(),
            Some("worktrees_sandbox")
        );
    }
}
