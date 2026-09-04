//! Mongo persistence for workspace aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::{SandboxId, WorkspaceId};
use layer2_core::sandbox::store::{
    Page, PageRequest, SandboxStoreError, WorkspaceStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Workspace;
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    i64_field_matches, is_duplicate_key, nullable, read_record, record_bson, require_version,
    signed, string_field_matches, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) fn workspace_indexes() -> Vec<IndexModel> {
    vec![IndexModel::builder()
        .keys(doc! { "sandbox_id": 1, "name": 1 })
        .options(
            IndexOptions::builder()
                .name("workspaces_sandbox_name_live".to_string())
                .unique(true)
                .partial_filter_expression(doc! { "removed_at": Bson::Null })
                .build(),
        )
        .build()]
}

pub(super) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_indexes(workspace_indexes())
        .await
        .map(|_| ())
        .map_err(|error| {
            layer2_core::StoreError::Unavailable(format!("workspace indexes: {error}"))
        })
}

pub(super) fn workspace_document(workspace: &Workspace) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": workspace.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "sandbox_id": workspace.sandbox_id.as_str(),
        "owner": workspace.owner.as_str(),
        "name": &workspace.name,
        "status": record_bson(&workspace.status).map_err(SandboxStoreError::Corrupt)?,
        "removed_at": nullable(workspace.removed_at.as_ref()),
        "revision": signed(workspace.revision, "workspace revision")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(workspace).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_workspace(document: &Document) -> Result<Workspace, SandboxStoreError> {
    let workspace: Workspace = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let status = record_bson(&workspace.status).map_err(SandboxStoreError::Corrupt)?;
    let revision =
        signed(workspace.revision, "workspace revision").map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", workspace.id.as_str())
        || !string_field_matches(document, "sandbox_id", workspace.sandbox_id.as_str())
        || !string_field_matches(document, "owner", workspace.owner.as_str())
        || !string_field_matches(document, "name", &workspace.name)
        || document.get("status") != Some(&status)
        || document.get("removed_at") != Some(&nullable(workspace.removed_at.as_ref()))
        || !i64_field_matches(document, "revision", revision)
    {
        return Err(SandboxStoreError::Corrupt(
            "workspace query fields disagree with its aggregate".into(),
        ));
    }
    Ok(workspace)
}

fn replacement_fields(workspace: &Workspace) -> Result<Document, SandboxStoreError> {
    let mut document = workspace_document(workspace)?;
    document.remove("_id");
    Ok(document)
}

async fn classify_miss(
    collection: &Collection<Document>,
    id: &WorkspaceId,
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
impl WorkspaceStore for MongoSandboxStore {
    async fn create_workspace(&self, workspace: Workspace) -> Result<(), SandboxStoreError> {
        if workspace.revision != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        self.workspaces
            .insert_one(workspace_document(&workspace)?)
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

    async fn get_workspace(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<Workspace>, SandboxStoreError> {
        self.workspaces
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .as_ref()
            .map(read_workspace)
            .transpose()
    }

    async fn save_workspace(
        &self,
        expected_revision: u64,
        workspace: Workspace,
    ) -> Result<(), SandboxStoreError> {
        if workspace.revision
            != expected_revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?
        {
            return Err(SandboxStoreError::Conflict);
        }
        let result = self
            .workspaces
            .update_one(
                doc! {
                    "_id": workspace.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": signed(expected_revision, "expected workspace revision")
                        .map_err(|_| SandboxStoreError::Conflict)?,
                },
                doc! { "$set": replacement_fields(&workspace)? },
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
            Err(classify_miss(&self.workspaces, &workspace.id).await?)
        }
    }

    async fn list_workspaces(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Workspace>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "sandbox_id": sandbox_id.as_str() };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .workspaces
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
            items.push(read_workspace(&document)?);
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
    fn workspace_index_is_unique_only_for_unremoved_names() {
        let index = workspace_indexes().remove(0);
        assert_eq!(index.keys, doc! { "sandbox_id": 1, "name": 1 });
        let options = index.options.unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression,
            Some(doc! { "removed_at": Bson::Null })
        );
    }
}
