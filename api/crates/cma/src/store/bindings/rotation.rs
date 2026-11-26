use super::*;
use crate::sandbox::ManagedSandboxCredentials;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::sandboxes::{read_sandbox, sandbox_replacement_fields};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::binding::ProvidedCredential;
use layer2_core::CoreError;

impl MongoBindingCredentials {
    pub(super) async fn rotate(
        &self,
        binding: &ProviderBinding,
        credential: &ProvidedCredential,
    ) -> Result<ProviderBinding, CoreError> {
        self.store
            .bindings
            .safety
            .require_keys()
            .map_err(unavailable)?;
        let next = binding.rotate_credential(credential.sha256(), layer2_core::now_rfc3339())?;
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = async {
            self.fence(binding, &mut session).await?;
            let document = self.store.sandboxes.find_one(doc! {"_id": binding.sandbox_id.as_str()})
                .session(&mut session).await.map_err(unavailable)?.ok_or_else(conflict)?;
            let sandbox = read_sandbox(&document).map_err(unavailable)?;
            let mut material = ManagedSandboxCredentials::binding_material(&next, credential)?;
            material.authority.generation = sandbox.credential.next_generation().ok_or_else(conflict)?;
            let updated = next.admit_credential_delivery(&sandbox, material.authority.clone())?;
            let database = self.store.bindings.records.namespace().db;
            self.custody.retain_binding_material(&database, &material, &mut session).await?;
            let result = self.store.sandboxes.update_one(crate::store::work_schema::query(doc! {"_id": sandbox.id.as_str(), "revision": integer(sandbox.revision).map_err(unavailable)?,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION}), crate::store::work_lease_update::replacement(sandbox_replacement_fields(&updated).map_err(unavailable)?))
                .session(&mut session).await.map_err(unavailable)?;
            if result.matched_count != 1 { return Err(conflict()); }
            crate::store::sandbox_usage::save::record_saved(&self.store, &mut session, &updated, sandbox.revision).await
                .map_err(|_| unavailable("usage persistence failed"))?;
            let mut document = binding_document(&next).map_err(unavailable)?;
            document.remove("_id");
            self.store.bindings.records.update_one(doc! {"_id": &binding.binding_id}, doc! {"$set": document, "$addToSet": {"key_history": {"$each": [&binding.key_sha256, &next.key_sha256]}}})
                .session(&mut session).await.map_err(unavailable)?;
            Ok(())
        }.await;
        if let Err(error) = result {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(next),
            _ => Err(unavailable("binding rotation commit was not acknowledged")),
        }
    }
}

fn unavailable(_: impl std::fmt::Display) -> CoreError {
    CoreError::RuntimeUnavailable("binding credential rotation unavailable".into())
}
fn conflict() -> CoreError {
    CoreError::InvalidState("binding credential rotation changed".into())
}
