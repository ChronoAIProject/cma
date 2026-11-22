//! Re-read the hierarchy without rebasing the original binding or credential intent.
use super::*;
use crate::store::sandboxes::read_sandbox;
use crate::store::workspaces::read_workspace;
use layer2_core::profiles::WorkspaceProfileSnapshot;
use layer2_core::sandbox::{
    RuntimeBinding, SandboxCredentialAuthority, WorkspaceOrigin, WorkspaceSource,
};
use mongodb::ClientSession;

#[derive(PartialEq, Eq)]
struct RecoverySandbox {
    runtime: Option<RuntimeBinding>,
    runtime_generation_high_water: u64,
    credential: Option<SandboxCredentialAuthority>,
}

#[derive(PartialEq, Eq)]
struct RecoveryWorkspace {
    origin: WorkspaceOrigin,
    profile: WorkspaceProfileSnapshot,
    path: String,
    source: WorkspaceSource,
    resolved_commit: Option<String>,
}

#[derive(Default)]
pub(super) struct RecoveryParents {
    sandbox: Option<RecoverySandbox>,
    workspace: Option<RecoveryWorkspace>,
}

fn retain_authority<T: Eq>(original: &mut Option<T>, observed: T) -> Result<(), CoreError> {
    match original {
        Some(original) if *original != observed => return Err(conflict()),
        Some(_) => {}
        None => *original = Some(observed),
    }
    Ok(())
}

impl MongoBindingCredentials {
    pub(super) async fn recover_in(
        &self,
        binding: &ProviderBinding,
        credential: Option<&ProvidedCredential>,
        at: &str,
        captured: &mut RecoveryParents,
        session: &mut ClientSession,
    ) -> Result<ProviderBinding, BindingTransactionError> {
        let sandbox_document = self
            .store
            .sandboxes
            .find_one(doc! {"_id": binding.sandbox_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(BindingTransactionError::Driver)?
            .ok_or_else(conflict)?;
        let sandbox = read_sandbox(&sandbox_document).map_err(unavailable)?;
        // Keep the observed Sandbox even if the Workspace read aborts this transaction.
        retain_authority(
            &mut captured.sandbox,
            RecoverySandbox {
                runtime: sandbox.runtime.clone(),
                runtime_generation_high_water: sandbox.lifecycle.runtime_generation_high_water,
                credential: sandbox.credential.current.clone(),
            },
        )?;
        let workspace_document = self
            .store
            .workspaces
            .find_one(doc! {"_id": binding.workspace_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(BindingTransactionError::Driver)?
            .ok_or_else(conflict)?;
        let workspace = read_workspace(&workspace_document).map_err(unavailable)?;
        retain_authority(
            &mut captured.workspace,
            RecoveryWorkspace {
                origin: workspace.origin.clone(),
                profile: workspace.profile.clone(),
                path: workspace.path.clone(),
                source: workspace.source.clone(),
                resolved_commit: workspace.resolved_commit.clone(),
            },
        )?;
        // Conversation writes also touch this fence. Retirement or a key
        // rotation cannot commit between admission and the readiness CAS.
        self.fence_transaction(binding, session).await?;
        let database = self.store.bindings.records.namespace().db;
        let retained = self
            .custody
            .current_binding_credential_transaction(&database, &sandbox, session)
            .await?;
        if credential.is_some_and(|provided| provided.expose() != retained.expose()) {
            return Err(conflict().into());
        }
        let next =
            binding.retry_recovered_hierarchy(&sandbox, &workspace, &retained, at.to_owned())?;
        if self
            .store
            .bindings
            .conversations
            .find_one(doc! {"binding_id": &binding.binding_id, "pending": true})
            .session(&mut *session)
            .await
            .map_err(BindingTransactionError::Driver)?
            .is_some()
        {
            return Err(conflict().into());
        }
        if self
            .store
            .agents
            .find_one(doc! {
                "sandbox_id": sandbox.id.as_str(),
                "$or": [{"work_pending": true}, {"work_lease_expires_at": {"$ne": null}},
                    {"record.work.cleanup_debt.0": {"$exists": true}}],
            })
            .session(&mut *session)
            .await
            .map_err(BindingTransactionError::Driver)?
            .is_some()
            || self
                .store
                .worktrees
                .find_one(doc! {
                    "sandbox_id": sandbox.id.as_str(), "status": {"$in": ["creating", "removing"]},
                })
                .session(&mut *session)
                .await
                .map_err(BindingTransactionError::Driver)?
                .is_some()
        {
            return Err(conflict().into());
        }
        // These writes serialize with lifecycle admission, including the
        // out-of-record cleanup reservations used by workspace reuse.
        for (collection, id, revision) in [
            (&self.store.sandboxes, sandbox.id.as_str(), sandbox.revision),
            (
                &self.store.workspaces,
                workspace.id.as_str(),
                workspace.revision,
            ),
        ] {
            let result = collection
                .update_one(
                    crate::store::work_schema::for_collection(
                        collection,
                        doc! {
                            "_id": id, "revision": integer(revision).map_err(unavailable)?,
                            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                            "channel_reuse_cleanup_execution": {"$exists": false},
                            "owned_workspace_cleanup_admission_v1": {"$exists": false},
                        },
                    ),
                    doc! {"$inc": {"binding_recovery_admission_fence": 1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(BindingTransactionError::Driver)?;
            if result.matched_count != 1 {
                return Err(conflict().into());
            }
        }
        // New child admission also writes the parent. Fence retained live
        // children so independently scheduled cleanup cannot race this
        // readiness decision; terminal records remain completely untouched.
        for (collection, terminal) in [
            (&self.store.agents, "deleted"),
            (&self.store.worktrees, "removed"),
        ] {
            collection
                .update_many(
                    doc! {
                        "sandbox_id": sandbox.id.as_str(), "status": {"$ne": terminal},
                    },
                    doc! {"$inc": {"binding_recovery_admission_fence": 1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(BindingTransactionError::Driver)?;
        }
        validate_binding_write(Some(binding), Some(binding.revision), &next)
            .map_err(unavailable)?;
        let mut document = binding_document(&next).map_err(unavailable)?;
        document.remove("_id");
        let changed = self
            .store
            .bindings
            .records
            .update_one(
                doc! {
                    "_id": &binding.binding_id,
                    "revision": integer(binding.revision).map_err(unavailable)?,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                },
                doc! {"$set": document},
            )
            .session(&mut *session)
            .await
            .map_err(BindingTransactionError::Driver)?;
        if changed.matched_count != 1 {
            return Err(conflict().into());
        }
        Ok(next)
    }
}
