//! Mongo persistence for workspace aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::{SandboxId, WorkspaceId};
use layer2_core::sandbox::store::{
    Page, PageRequest, SandboxStoreError, SandboxTransaction, SandboxTransactionStore,
    WorkspaceStore, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{SandboxEventKind, Workspace, WorkspaceOrigin, WorkspaceStatus};
use mongodb::bson::{doc, Bson, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::sandbox_mongo::{
    i64_field_matches, is_duplicate_key, nullable, record_bson, signed, string_field_matches,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

#[cfg(test)]
mod mongo_integration_tests;

pub(super) fn workspace_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "name": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_name_live".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "removed_at": Bson::Null })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "origin_index": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_profile_origin".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "sandbox_profile" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "origin_launch_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_trigger_origin".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "trigger" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "origin_operation_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_lifecycle_origin".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "lifecycle" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_due_work".to_string())
                    .build(),
            )
            .build(),
        // Maintenance fences every child, including removed and manual Workspaces.
        // None of the partial indexes above covers that complete-child lookup.
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox".to_string())
                    .build(),
            )
            .build(),
    ]
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
    let (origin_type, origin_index, origin_launch_id) = origin_fields(&workspace.origin);
    let lease_expires_at = workspace.work.lease.as_ref().map(|lease| &lease.expires_at);
    Ok(doc! {
        "_id": workspace.id.as_str(),
        "document_schema_version": super::work_schema::version(workspace.work.authority_protocol, false),
        "sandbox_id": workspace.sandbox_id.as_str(),
        "owner": workspace.owner.as_str(),
        "name": &workspace.name,
        "origin_type": origin_type,
        "origin_index": origin_index,
        "origin_launch_id": origin_launch_id,
        "origin_operation_id": operation_origin(&workspace.origin),
        "status": record_bson(&workspace.status).map_err(SandboxStoreError::Corrupt)?,
        "removed_at": nullable(workspace.removed_at.as_ref()),
        "work_pending": workspace.work.step.is_pending(),
        "work_ready_at": &workspace.work.ready_at,
        "work_lease_expires_at": nullable(lease_expires_at),
        "next_worktree_number": signed(
            workspace.next_worktree_number,
            "workspace next worktree number",
        )
        .map_err(SandboxStoreError::Corrupt)?,
        "revision": signed(workspace.revision, "workspace revision")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(workspace).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_workspace(document: &Document) -> Result<Workspace, SandboxStoreError> {
    let workspace: Workspace =
        super::work_schema::read(document).map_err(SandboxStoreError::Corrupt)?;
    let status = record_bson(&workspace.status).map_err(SandboxStoreError::Corrupt)?;
    let (origin_type, origin_index, origin_launch_id) = origin_fields(&workspace.origin);
    let revision =
        signed(workspace.revision, "workspace revision").map_err(SandboxStoreError::Corrupt)?;
    let next_worktree_number = signed(
        workspace.next_worktree_number,
        "workspace next worktree number",
    )
    .map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", workspace.id.as_str())
        || !string_field_matches(document, "sandbox_id", workspace.sandbox_id.as_str())
        || !string_field_matches(document, "owner", workspace.owner.as_str())
        || !string_field_matches(document, "name", &workspace.name)
        || !string_field_matches(document, "origin_type", origin_type)
        || document.get("origin_index") != Some(&origin_index)
        || document.get("origin_launch_id") != Some(&origin_launch_id)
        || document.get("origin_operation_id").unwrap_or(&Bson::Null)
            != &operation_origin(&workspace.origin)
        || document.get("status") != Some(&status)
        || document.get("removed_at") != Some(&nullable(workspace.removed_at.as_ref()))
        || document.get_bool("work_pending").ok() != Some(workspace.work.step.is_pending())
        || !string_field_matches(document, "work_ready_at", &workspace.work.ready_at)
        || document.get("work_lease_expires_at")
            != Some(&nullable(
                workspace.work.lease.as_ref().map(|lease| &lease.expires_at),
            ))
        || !i64_field_matches(document, "next_worktree_number", next_worktree_number)
        || !i64_field_matches(document, "revision", revision)
    {
        return Err(SandboxStoreError::Corrupt(
            "workspace query fields disagree with its aggregate".into(),
        ));
    }
    Ok(workspace)
}

pub(super) fn workspace_replacement_fields(
    workspace: &Workspace,
) -> Result<Document, SandboxStoreError> {
    let mut document = workspace_document(workspace)?;
    document.remove("_id");
    Ok(document)
}

fn origin_fields(origin: &WorkspaceOrigin) -> (&'static str, Bson, Bson) {
    match origin {
        WorkspaceOrigin::Manual => ("manual", Bson::Null, Bson::Null),
        WorkspaceOrigin::SandboxProfile { index } => (
            "sandbox_profile",
            Bson::Int64(i64::from(*index)),
            Bson::Null,
        ),
        WorkspaceOrigin::Trigger { launch_id } => {
            ("trigger", Bson::Null, Bson::String(launch_id.clone()))
        }
        WorkspaceOrigin::Lifecycle { .. } => ("lifecycle", Bson::Null, Bson::Null),
    }
}

fn operation_origin(origin: &WorkspaceOrigin) -> Bson {
    match origin {
        WorkspaceOrigin::Lifecycle { operation_id } => Bson::String(operation_id.clone()),
        _ => Bson::Null,
    }
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
            super::work_schema::require(&document).map_err(SandboxStoreError::Corrupt)?;
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
        super::native_retirement_action::insert_child(
            self,
            &self.workspaces,
            &workspace.sandbox_id,
            workspace_document(&workspace)?,
        )
        .await
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
        let current = self
            .get_workspace(&workspace.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if current.revision != expected_revision {
            return Err(SandboxStoreError::Conflict);
        }
        layer2_core::sandbox::ProvisioningExecution::validate_successor(
            current.work.execution,
            workspace.work.execution,
        )
        .map_err(|_| SandboxStoreError::Conflict)?;
        if workspace.revision
            != expected_revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?
        {
            return Err(SandboxStoreError::Conflict);
        }
        if workspace.status == WorkspaceStatus::Ready {
            if workspace.owner != current.owner || workspace.sandbox_id != current.sandbox_id {
                return Err(SandboxStoreError::Conflict);
            }
            if current.status != WorkspaceStatus::Ready {
                let event_at =
                    workspace
                        .ready_at
                        .clone()
                        .ok_or(SandboxStoreError::InvalidTransaction(
                            "ready workspace timestamp missing",
                        ))?;
                let resolved_commit = workspace.resolved_commit.clone().ok_or(
                    SandboxStoreError::InvalidTransaction("ready workspace commit missing"),
                )?;
                return self
                    .commit(SandboxTransaction::SaveWorkspaceAndAppend {
                        repository_runtime: None,
                        expected_revision,
                        event_at,
                        event: SandboxEventKind::WorkspaceReady {
                            workspace_id: workspace.id.clone(),
                            resolved_commit,
                        },
                        workspace: Box::new(workspace),
                    })
                    .await;
            }
        }
        if layer2_core::sandbox::store::WorkAuthority::active() {
            return super::work_publication::workspace::save(self, &current, &workspace).await;
        }
        let result = self
            .workspaces
            .update_one(
                crate::store::work_schema::query(doc! {
                    "_id": workspace.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                "owned_workspace_cleanup_admission_v1": { "$exists": false },
                    "revision": signed(expected_revision, "expected workspace revision")
                        .map_err(|_| SandboxStoreError::Conflict)?,
                }),
                crate::store::work_lease_update::replacement(workspace_replacement_fields(
                    &workspace,
                )?),
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
            .find(crate::store::work_schema::query(filter))
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
    fn workspace_indexes_cover_live_names_durable_origins_and_due_work() {
        let indexes = workspace_indexes();
        assert_eq!(indexes[0].keys, doc! { "sandbox_id": 1, "name": 1 });
        let options = indexes[0].options.as_ref().unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression.as_ref(),
            Some(&doc! { "removed_at": Bson::Null })
        );
        assert_eq!(indexes[1].keys, doc! { "sandbox_id": 1, "origin_index": 1 });
        let options = indexes[1].options.as_ref().unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression.as_ref(),
            Some(&doc! { "origin_type": "sandbox_profile" })
        );
        assert_eq!(
            indexes[2].keys,
            doc! { "sandbox_id": 1, "origin_launch_id": 1 }
        );
        let options = indexes[2].options.as_ref().unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression.as_ref(),
            Some(&doc! { "origin_type": "trigger" })
        );
        assert_eq!(
            indexes[3].keys,
            doc! { "sandbox_id": 1, "origin_operation_id": 1 }
        );
        let options = indexes[3].options.as_ref().unwrap();
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression.as_ref(),
            Some(&doc! { "origin_type": "lifecycle" })
        );
        assert_eq!(
            indexes[4].keys,
            doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 }
        );
        assert_eq!(
            indexes[4].options.as_ref().unwrap().name.as_deref(),
            Some("workspaces_due_work")
        );
    }
}
