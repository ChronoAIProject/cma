use super::*;
use crate::sandbox::ManagedSandboxCredentials;
use crate::store::sandbox_credentials::MongoSandboxCredentialStore;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::binding::{BindingCredentialCustody, ProvidedCredential};
use layer2_core::{CoreError, SandboxId, ServiceId, SubjectId};
use mongodb::ClientSession;
use std::sync::Arc;

pub(crate) struct MongoBindingCredentials {
    pub(super) store: Arc<MongoSandboxStore>,
    pub(super) custody: Arc<MongoSandboxCredentialStore>,
}

impl MongoBindingCredentials {
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        custody: Arc<MongoSandboxCredentialStore>,
    ) -> Arc<Self> {
        Arc::new(Self { store, custody })
    }

    pub(super) async fn fence(
        &self,
        expected: &ProviderBinding,
        session: &mut ClientSession,
    ) -> Result<(), CoreError> {
        self.fence_transaction(expected, session)
            .await
            .map_err(|error| error.into_core(unavailable))
    }

    /// Preserve transaction abort classification for callers that own retry.
    pub(super) async fn fence_transaction(
        &self,
        expected: &ProviderBinding,
        session: &mut ClientSession,
    ) -> Result<(), BindingTransactionError> {
        let records = &self.store.bindings.records;
        let result = records.update_one(doc! {"_id": &expected.binding_id,
            "revision": integer(expected.revision).map_err(unavailable)?, "document_schema_version": DOCUMENT_SCHEMA_VERSION},
            doc! {"$inc": {"admission_fence": 1_i64}}).session(&mut *session).await.map_err(BindingTransactionError::Driver)?;
        if result.matched_count != 1 {
            return Err(CoreError::InvalidState("binding credential fence changed".into()).into());
        }
        let actual = records
            .find_one(doc! {"_id": &expected.binding_id})
            .session(&mut *session)
            .await
            .map_err(BindingTransactionError::Driver)?
            .map(read_binding)
            .transpose()
            .map_err(unavailable)?;
        if actual.as_ref() != Some(expected) {
            return Err(
                CoreError::InvalidState("binding credential identity changed".into()).into(),
            );
        }
        Ok(())
    }

    async fn apply(
        &self,
        binding: &ProviderBinding,
        credential: Option<&ProvidedCredential>,
        id: &SandboxId,
    ) -> Result<(), CoreError> {
        if credential.is_some() {
            self.store
                .bindings
                .safety
                .require_keys()
                .map_err(unavailable)?;
        }
        let mut session = self
            .store
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = async {
            self.fence(binding, &mut session).await?;
            let database = self.store.bindings.records.namespace().db;
            if let Some(credential) = credential {
                binding.require_credential_admission(credential)?;
                let material = ManagedSandboxCredentials::binding_material(binding, credential)?;
                self.custody
                    .retain_binding_material(&database, &material, &mut session)
                    .await
            } else {
                binding.require_credential_retirement(id)?;
                if let Some(document) = self
                    .store
                    .sandboxes
                    .find_one(doc! {"_id": id.as_str()})
                    .session(&mut session)
                    .await
                    .map_err(unavailable)?
                {
                    let sandbox =
                        crate::store::sandboxes::read_sandbox(&document).map_err(unavailable)?;
                    if sandbox.owner != binding.owner
                        || sandbox.status != layer2_core::sandbox::SandboxStatus::Retired
                    {
                        return Err(CoreError::InvalidState(
                            "binding sandbox has not retired".into(),
                        ));
                    }
                }
                self.custody
                    .forget_binding_material(&database, id, &binding.owner, &mut session)
                    .await
            }
        }
        .await;
        if let Err(error) = result {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Retry(_) | CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => Err(
                unavailable("binding credential commit was not acknowledged"),
            ),
        }
    }
}

#[async_trait::async_trait]
impl BindingCredentialCustody for MongoBindingCredentials {
    fn validate(&self, credential: &ProvidedCredential) -> Result<(), CoreError> {
        crate::auth::keys::KeyIssuer::provided(credential.expose())
            .map(|_| ())
            .map_err(|_| CoreError::InvalidRequest("invalid supplied agent key".into()))
    }

    async fn retain_binding(
        &self,
        binding: &ProviderBinding,
        credential: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        self.apply(binding, Some(credential), &binding.sandbox_id)
            .await
    }

    async fn rotate_binding(
        &self,
        binding: &ProviderBinding,
        credential: &ProvidedCredential,
    ) -> Result<ProviderBinding, CoreError> {
        self.rotate(binding, credential).await
    }

    async fn retry_recovered_binding(
        &self,
        binding: &ProviderBinding,
        credential: &ProvidedCredential,
    ) -> Result<ProviderBinding, CoreError> {
        self.retry_recovered(binding, Some(credential)).await
    }

    async fn recover_binding(
        &self,
        binding: &ProviderBinding,
    ) -> Result<ProviderBinding, CoreError> {
        self.retry_recovered(binding, None).await
    }

    async fn prepare_retired_binding(
        &self,
        intent: &layer2_core::sandbox::binding::BindingPreparationIntent,
        credential: &ProvidedCredential,
    ) -> Result<layer2_core::sandbox::binding::BindingPreparationReceipt, CoreError> {
        self.prepare_retired(intent, credential).await
    }

    async fn forget_binding(
        &self,
        binding: &ProviderBinding,
        id: &SandboxId,
    ) -> Result<(), CoreError> {
        self.apply(binding, None, id).await
    }

    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        Err(CoreError::InvalidState(
            "binding credential admission requires its reservation".into(),
        ))
    }
}

fn unavailable(_: impl std::fmt::Display) -> CoreError {
    CoreError::RuntimeUnavailable("binding credential custody is unavailable".into())
}
