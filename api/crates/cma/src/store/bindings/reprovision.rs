//! The only transaction permitted to replace an explicitly retired binding home.
//! No caller uses this until the compatible Gateway writer is enabled.
use super::*;
use crate::sandbox::ManagedSandboxCredentials;
use crate::store::sandbox_mongo::{
    commit_transaction, read_record, record_bson, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::binding::{
    BindingPreparationIntent, BindingPreparationReceipt, ProvidedCredential,
};
use layer2_core::CoreError;
use mongodb::ClientSession;

mod children;

pub(super) const PREPARATIONS: &str = "provider_binding_preparations";

impl MongoBindingCredentials {
    pub(super) async fn prepare_retired(
        &self,
        intent: &BindingPreparationIntent,
        credential: &ProvidedCredential,
    ) -> Result<BindingPreparationReceipt, CoreError> {
        use layer2_core::sandbox::binding::BindingCredentialCustody;
        self.validate(credential)?;
        intent.validate()?;
        if let Some(receipt) = self.preparation_receipt(intent, credential).await? {
            return Ok(receipt);
        }
        intent.require_current(
            layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
                .and_then(|value| value.try_into().ok())
                .ok_or_else(conflict)?,
        )?;
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = self
            .apply_preparation(intent, credential, &mut session)
            .await;
        match result {
            Ok(receipt) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => Ok(receipt),
                _ => self
                    .preparation_receipt(intent, credential)
                    .await?
                    .ok_or_else(|| unavailable("unacknowledged preparation commit")),
            },
            Err(error) => {
                let _ = session.abort_guarded().await;
                // A concurrent exact winner is replayed; never re-inspect a
                // newer generation or choose another pair of resource IDs.
                match self.preparation_receipt(intent, credential).await? {
                    Some(receipt) => Ok(receipt),
                    None => Err(error),
                }
            }
        }
    }

    async fn preparation_receipt(
        &self,
        intent: &BindingPreparationIntent,
        credential: &ProvidedCredential,
    ) -> Result<Option<BindingPreparationReceipt>, CoreError> {
        self.store
            .bindings
            .preparations
            .find_one(doc! {"_id": intent.operation_id()?})
            .await
            .map_err(unavailable)?
            .map(|document| {
                let receipt = read_receipt(&document)?;
                receipt.matches(intent)?;
                if receipt.previous.key_sha256 != credential.sha256() {
                    return Err(conflict());
                }
                Ok(receipt)
            })
            .transpose()
    }

    async fn apply_preparation(
        &self,
        intent: &BindingPreparationIntent,
        credential: &ProvidedCredential,
        session: &mut ClientSession,
    ) -> Result<BindingPreparationReceipt, CoreError> {
        self.store
            .bindings
            .safety
            .require_preparations()
            .map_err(unavailable)?;
        self.store
            .bindings
            .safety
            .require_keys()
            .map_err(unavailable)?;
        crate::store::admission_checks::require_fresh_home(
            &self.store.budget_settings,
            true,
            session,
        )
        .await
        .map_err(|error| match error {
            layer2_core::sandbox::store::SandboxStoreError::AdmissionPaused => {
                CoreError::AdmissionPaused
            }
            layer2_core::sandbox::store::SandboxStoreError::GlobalCapacityReached => {
                CoreError::AdmissionCapacity(layer2_core::admission::CapacityKind::RetainedHomes)
            }
            other => unavailable(other),
        })?;
        let binding = self
            .store
            .bindings
            .records
            .find_one(doc! {"_id": &intent.binding_id})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .map(read_binding)
            .transpose()
            .map_err(unavailable)?
            .ok_or_else(conflict)?;
        if binding.owner != intent.owner
            || binding.manager != intent.manager
            || binding.generation != intent.generation
            || binding.revision != intent.binding_revision
        {
            return Err(conflict());
        }
        self.fence(&binding, session).await?;
        let sandbox = self
            .store
            .sandboxes
            .find_one(doc! {"_id": binding.sandbox_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .map(|value| crate::store::sandboxes::read_sandbox(&value))
            .transpose()
            .map_err(unavailable)?
            .ok_or_else(conflict)?;
        let workspace = self
            .store
            .workspaces
            .find_one(doc! {"_id": binding.workspace_id.as_str()})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .map(|value| crate::store::workspaces::read_workspace(&value))
            .transpose()
            .map_err(unavailable)?
            .ok_or_else(conflict)?;
        let receipt = binding.prepare_retired_hierarchy(
            intent,
            &sandbox,
            &workspace,
            credential,
            layer2_core::now_rfc3339(),
        )?;
        // Exact parent writes serialize child admission and out-of-record
        // cleanup reservations. Original aggregates remain byte-identical.
        for (collection, id, revision) in [
            (&self.store.sandboxes, sandbox.id.as_str(), sandbox.revision),
            (
                &self.store.workspaces,
                workspace.id.as_str(),
                workspace.revision,
            ),
        ] {
            let changed = collection
                .update_one(
                    doc! {
                        "_id":id, "revision":integer(revision).map_err(unavailable)?,
                        "document_schema_version":DOCUMENT_SCHEMA_VERSION,
                        "channel_reuse_cleanup_execution":{"$exists":false},
                        "owned_workspace_cleanup_admission_v1":{"$exists":false},
                    },
                    doc! {"$inc":{"binding_preparation_fence":1_i64}},
                )
                .session(&mut *session)
                .await
                .map_err(unavailable)?;
            if changed.matched_count != 1 {
                return Err(conflict());
            }
        }
        self.require_terminal_children(&binding, session).await?;
        // This immutable receipt archives the original generation. Its unique
        // index prevents distinct events reserving the same lineage twice.
        self.store
            .bindings
            .preparations
            .insert_one(receipt_document(&receipt)?)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let material = ManagedSandboxCredentials::binding_material(&receipt.reserved, credential)?;
        let database = self.store.bindings.records.namespace().db;
        self.custody
            .retain_binding_material(&database, &material, session)
            .await?;
        let mut replacement = binding_document(&receipt.reserved).map_err(unavailable)?;
        replacement.remove("_id");
        // Generic validate_binding_write still forbids generation changes.
        // Only this validated receipt permits the transition in this transaction.
        receipt.validate()?;
        let changed = self.store.bindings.records.update_one(doc! {
            "_id":&binding.binding_id, "revision":integer(binding.revision).map_err(unavailable)?,
            "generation":integer(binding.generation).map_err(unavailable)?,
            "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        }, doc! {"$set":replacement}).session(&mut *session).await.map_err(unavailable)?;
        if changed.matched_count != 1 {
            return Err(conflict());
        }
        Ok(receipt)
    }
}

fn receipt_document(receipt: &BindingPreparationReceipt) -> Result<Document, CoreError> {
    receipt.validate()?;
    Ok(
        doc! {"_id":&receipt.operation_id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "binding_id":&receipt.previous.binding_id,"owner":receipt.previous.owner.as_str(),
        "generation":integer(receipt.previous.generation).map_err(unavailable)?,
        "fingerprint":receipt.intent.fingerprint()?,"record":record_bson(receipt).map_err(unavailable)?},
    )
}
fn read_receipt(document: &Document) -> Result<BindingPreparationReceipt, CoreError> {
    let receipt: BindingPreparationReceipt = read_record(document).map_err(unavailable)?;
    let expected = receipt_document(&receipt)?;
    if expected
        .iter()
        .any(|(key, value)| document.get(key) != Some(value))
    {
        return Err(conflict());
    }
    Ok(receipt)
}
fn conflict() -> CoreError {
    CoreError::InvalidState("binding preparation authority changed or is busy".into())
}
fn unavailable(_: impl std::fmt::Display) -> CoreError {
    CoreError::RuntimeUnavailable("binding preparation admission unavailable".into())
}

#[cfg(test)]
mod tests;
