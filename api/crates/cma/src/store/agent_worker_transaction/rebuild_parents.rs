//! Serialize a rebuilt publication against its freshly verified parent revisions.
use super::*;

pub(super) async fn fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    parents: &AgentWorkPublicationParents,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    for (collection, id, revision) in [
        (
            &store.sandboxes,
            agent.sandbox_id.as_str(),
            parents.sandbox_revision,
        ),
        (
            &store.workspaces,
            agent.workspace_id.as_str(),
            parents.workspace_revision,
        ),
        (
            &store.worktrees,
            agent.worktree_id.as_str(),
            parents.worktree_revision,
        ),
    ] {
        let result = scope
            .update_one(
                collection,
                doc! {
                    "_id": id, "owner": agent.owner.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": signed(revision, "publication parent revision")
                        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                    "owned_workspace_cleanup_admission_v1": { "$exists": false },
                },
                doc! { "$inc": { "runtime_writer_claim_fence": 1_i64 } },
            )
            .await
            .map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}
