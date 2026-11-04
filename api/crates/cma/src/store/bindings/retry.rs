use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::sandboxes::read_sandbox;
use crate::store::workspaces::read_workspace;
use layer2_core::sandbox::binding::ProvidedCredential;
use layer2_core::CoreError;

impl MongoBindingCredentials {
    pub(super) async fn retry_recovered(
        &self,
        binding: &ProviderBinding,
        credential: Option<&ProvidedCredential>,
    ) -> Result<ProviderBinding, CoreError> {
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = async {
            // Conversation writes also touch this fence. Retirement or a key
            // rotation cannot commit between admission and the readiness CAS.
            self.fence(binding, &mut session).await?;
            let sandbox_document = self
                .store
                .sandboxes
                .find_one(doc! {"_id": binding.sandbox_id.as_str()})
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .ok_or_else(conflict)?;
            let workspace_document = self
                .store
                .workspaces
                .find_one(doc! {"_id": binding.workspace_id.as_str()})
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .ok_or_else(conflict)?;
            let sandbox = read_sandbox(&sandbox_document).map_err(unavailable)?;
            let workspace = read_workspace(&workspace_document).map_err(unavailable)?;
            let database = self.store.bindings.records.namespace().db;
            let retained = self
                .custody
                .current_binding_credential(&database, &sandbox, &mut session)
                .await?;
            if credential.is_some_and(|provided| provided.expose() != retained.expose()) {
                return Err(conflict());
            }
            let next = binding.retry_recovered_hierarchy(
                &sandbox,
                &workspace,
                &retained,
                layer2_core::now_rfc3339(),
            )?;
            if self
                .store
                .bindings
                .conversations
                .find_one(doc! {"binding_id": &binding.binding_id, "pending": true})
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .is_some()
            {
                return Err(conflict());
            }
            if self.store.agents.find_one(doc! {
                "sandbox_id": sandbox.id.as_str(),
                "$or": [{"work_pending": true}, {"work_lease_expires_at": {"$ne": null}},
                    {"record.work.cleanup_debt.0": {"$exists": true}}],
            }).session(&mut session).await.map_err(unavailable)?.is_some()
                || self.store.worktrees.find_one(doc! {
                    "sandbox_id": sandbox.id.as_str(), "status": {"$in": ["creating", "removing"]},
                }).session(&mut session).await.map_err(unavailable)?.is_some()
            {
                return Err(conflict());
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
                        doc! {
                            "_id": id, "revision": integer(revision).map_err(unavailable)?,
                            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                            "channel_reuse_cleanup_execution": {"$exists": false},
                            "owned_workspace_cleanup_admission_v1": {"$exists": false},
                        },
                        doc! {"$inc": {"binding_recovery_admission_fence": 1_i64}},
                    )
                    .session(&mut session)
                    .await
                    .map_err(unavailable)?;
                if result.matched_count != 1 {
                    return Err(conflict());
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
                    .session(&mut session)
                    .await
                    .map_err(unavailable)?;
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
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            if changed.matched_count != 1 {
                return Err(conflict());
            }
            Ok(next)
        }
        .await;
        let next = match result {
            Ok(next) => next,
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(next),
            _ => Err(unavailable("binding recovery commit was not acknowledged")),
        }
    }
}

fn conflict() -> CoreError {
    CoreError::InvalidState("binding recovery authority changed or is busy".into())
}
fn unavailable(_: impl std::fmt::Display) -> CoreError {
    CoreError::RuntimeUnavailable("binding recovery admission unavailable".into())
}
