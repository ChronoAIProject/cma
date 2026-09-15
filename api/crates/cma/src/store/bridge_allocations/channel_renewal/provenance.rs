use super::super::MongoBridgeAllocationStore;
use crate::{
    sandbox::{channel_credentials::SandboxCredentialProvenance, CredentialError},
    store::{durability::majority_collection, sandboxes::read_sandbox},
};
use layer2_core::SandboxId;
use mongodb::{
    bson::{doc, Document},
    Collection,
};

#[async_trait::async_trait]
impl SandboxCredentialProvenance for MongoBridgeAllocationStore {
    async fn require_ordinary(&self, sandbox_id: &SandboxId) -> Result<(), CredentialError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let sandboxes: Collection<Document> = majority_collection(&database, "sandboxes");
        let document = sandboxes
            .find_one(doc! {"_id":sandbox_id.as_str()})
            .await
            .map_err(|_| held())?
            .ok_or_else(held)?;
        let sandbox = read_sandbox(&document).map_err(|_| held())?;
        if sandbox.id != *sandbox_id
            || document.contains_key("channel_creation")
            || document.contains_key("channel_provisioning")
            || document.contains_key("channel_renewal")
        {
            return Err(held());
        }
        // An incomplete/corrupt resource-side provenance record must not
        // silently reclassify a Channel Sandbox into the ordinary token lane.
        if self
            .records
            .find_one(doc! {"sandbox_admission.sandbox_id":sandbox_id.as_str()})
            .await
            .map_err(|_| held())?
            .is_some()
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
