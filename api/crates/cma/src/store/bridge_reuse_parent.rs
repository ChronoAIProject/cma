//! Channel-created roots have a separate original-human credential lifecycle.
//! Reusing one must never silently route it through the ordinary parent worker.
use super::{
    durability::majority_collection,
    sandbox_mongo::{is_transient, start_transaction, TRANSACTION_ATTEMPTS},
    MongoSandboxStore,
};
use layer2_core::{bridge::allocation::AllocationError, sandbox::Sandbox};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

impl MongoSandboxStore {
    pub(crate) async fn require_ordinary_reuse_parent(
        &self,
        parent: &Sandbox,
    ) -> Result<(), AllocationError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = self.ordinary_reuse_parent(parent, &mut session).await;
            let _ = session.abort_transaction().await;
            match result {
                Ok(()) => return Ok(()),
                Err(ParentError::Core(error)) => return Err(error),
                Err(ParentError::Driver(error)) if is_transient(&error) => continue,
                Err(ParentError::Driver(_)) => return Err(AllocationError::Unavailable),
            }
        }
        Err(AllocationError::Unavailable)
    }
    pub(in crate::store) async fn ordinary_reuse_parent(
        &self,
        parent: &Sandbox,
        session: &mut ClientSession,
    ) -> Result<(), ParentError> {
        let row = self
            .sandboxes
            .find_one(doc! {"_id":parent.id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::IncompatibleTarget)?;
        if super::sandboxes::read_sandbox(&row).map_err(|_| AllocationError::Corrupt)? != *parent
            || row.contains_key("channel_creation")
            || row.contains_key("channel_provisioning")
            || row.contains_key("channel_renewal")
            || row.contains_key("channel_lifecycle_credential")
            || row.contains_key("channel_mint_reconciliation")
            || row.contains_key("channel_retirement")
            || row.contains_key("channel_cleanup")
        {
            return Err(AllocationError::IncompatibleTarget.into());
        }
        let database = self.client.database(&self.sandboxes.namespace().db);
        let allocations = majority_collection::<Document>(&database, "bridge_allocations");
        if allocations
            .find_one(doc! {"sandbox_admission.sandbox_id":parent.id.as_str()})
            .session(&mut *session)
            .await?
            .is_some()
        {
            return Err(AllocationError::IncompatibleTarget.into());
        }
        // Effectful callers also perform the shared root write fence in their
        // own transaction. This read-only setup check grants no later effect.
        Ok(())
    }
}
#[derive(Debug, thiserror::Error)]
pub(in crate::store) enum ParentError {
    #[error(transparent)]
    Core(#[from] AllocationError),
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}
