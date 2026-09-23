use super::MongoSandboxStore;
use crate::{
    sandbox::{legacy_credentials::SandboxCredentialProvenance, CredentialError},
    store::{durability::majority_collection, sandboxes::read_sandbox},
};
use layer2_core::SandboxId;
use mongodb::{
    bson::{doc, Document},
    Collection,
};

#[async_trait::async_trait]
impl SandboxCredentialProvenance for MongoSandboxStore {
    async fn require_ordinary(&self, sandbox_id: &SandboxId) -> Result<(), CredentialError> {
        let database = self.client.database(&self.sandboxes.namespace().db);
        let sandboxes: Collection<Document> = majority_collection(&database, "sandboxes");
        let document = sandboxes
            .find_one(doc! {"_id":sandbox_id.as_str()})
            .await
            .map_err(|_| held())?
            .ok_or_else(held)?;
        let sandbox = read_sandbox(&document).map_err(|_| held())?;
        if sandbox.id != *sandbox_id
            || document.contains_key("channel_renewal")
            || document.contains_key("channel_lifecycle_credential")
            || document.contains_key("channel_mint_reconciliation")
        {
            return Err(held());
        }
        // An incomplete/corrupt resource-side provenance record must not
        // silently reclassify a Channel Sandbox into the ordinary token lane.
        if crate::store::channel_storage_retention::required(
            &database,
            sandbox_id.as_str(),
            Some(&document),
            None,
        )
        .await
        .map_err(|_| held())?
        {
            return Err(held());
        }
        Ok(())
    }
}
fn held() -> CredentialError {
    CredentialError::Unavailable(
        "credential effects require the original Sandbox allocation and renewable human authority"
            .into(),
    )
}
