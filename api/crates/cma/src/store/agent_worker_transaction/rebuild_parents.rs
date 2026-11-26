//! Serialize a rebuilt publication against its freshly verified parent revisions.
use super::*;
use layer2_core::sandbox::store::AgentWorkPublicationLineage;
use layer2_core::sandbox::AgentLogKind;

pub(super) async fn fence(
    store: &MongoSandboxStore,
    agent: &Agent,
    mutation: &AgentWorkMutation,
    parents: &AgentWorkPublicationParents,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    let owner = Bson::String(agent.owner.as_str().to_owned());
    let workspace_owner = match parents.lineage {
        AgentWorkPublicationLineage::Authorized => owner.clone(),
        AgentWorkPublicationLineage::Rejected { workspace_foreign } => {
            if agent.status != AgentStatus::Starting
                || mutation.agent.status != AgentStatus::Failed
                || agent
                    .work
                    .cleanup_debt
                    .iter()
                    .any(|debt| debt.step == agent.work.step)
                || !matches!(mutation.agent_logs.as_slice(),
                    [AgentLogKind::WorkStepFailed { step, failure }]
                    if *step == agent.work.step && failure.class == "agent_lineage_invalid")
            {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
            if workspace_foreign {
                Bson::Document(doc! {"$type": "string", "$ne": agent.owner.as_str()})
            } else {
                owner.clone()
            }
        }
    };
    let refresh_workspace_wait = agent.work.step == AgentWorkStep::WaitForWorkspace
        && agent.has_no_runtime_effects()
        && parents.lineage == AgentWorkPublicationLineage::Authorized
        && (mutation.agent.work.step == AgentWorkStep::AddWorktree
            || persistence::is_effect_free_workspace_wait_requeue(mutation));
    for (collection, id, revision, owner, refresh) in [
        (
            &store.sandboxes,
            agent.sandbox_id.as_str(),
            parents.sandbox_revision,
            owner.clone(),
            refresh_workspace_wait,
        ),
        (
            &store.workspaces,
            agent.workspace_id.as_str(),
            parents.workspace_revision,
            workspace_owner,
            refresh_workspace_wait,
        ),
        (
            &store.worktrees,
            agent.worktree_id.as_str(),
            parents.worktree_revision,
            owner,
            false,
        ),
    ] {
        let revision = signed(revision, "publication parent revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
        let mut filter = doc! {
            "_id": id, "owner": owner,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": revision,
            "channel_reuse_cleanup_execution": { "$exists": false },
            "owned_workspace_cleanup_admission_v1": { "$exists": false },
        };
        let result = scope
            .update_one(
                collection,
                filter.clone(),
                doc! { "$inc": { "runtime_writer_claim_fence": 1_i64 } },
            )
            .await
            .map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            if refresh {
                // Only a newer revision under the same other guards permits a
                // rebuild. The worker must still revalidate parent compatibility.
                filter.insert("revision", doc! { "$gt": revision });
                if scope
                    .find_projected(collection, filter, doc! { "_id": 1 })
                    .await
                    .map_err(ApplyError::Driver)?
                    .is_some()
                {
                    return Err(ApplyError::StaleProposal);
                }
            }
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
    }
    Ok(())
}
