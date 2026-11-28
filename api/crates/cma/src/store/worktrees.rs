//! Mongo persistence for worktree aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::{WorkspaceId, WorktreeId};
use layer2_core::sandbox::store::{
    Page, PageRequest, SandboxStoreError, WorktreeStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Worktree;
use mongodb::bson::{doc, Document};
use mongodb::Collection;

use super::sandbox_mongo::{is_duplicate_key, signed, MongoSandboxStore};
use cma_am_sandbox_state::worktree::DOCUMENT_SCHEMA_VERSION;

pub(super) use cma_am_sandbox_state::worktree::{
    read_worktree, worktree_document, worktree_indexes, worktree_replacement_fields,
};

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(worktree_indexes())
        .await
        .map(|_| ())
        .map_err(|error| layer2_core::StoreError::Unavailable(format!("worktree indexes: {error}")))
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
            cma_am_mongo_primitives::require_version(&document, DOCUMENT_SCHEMA_VERSION)
                .map_err(SandboxStoreError::Corrupt)?;
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
