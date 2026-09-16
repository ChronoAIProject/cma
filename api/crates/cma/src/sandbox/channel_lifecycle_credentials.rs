//! A paused Channel revives with a fresh scoped key under its original human.
//! The canonical lifecycle worker owns runtime/Agent restoration throughout.
mod driver;
pub(crate) mod model;
use crate::{
    bridge::provisioning::revival::ChannelRevivalAuthoritySource,
    sandbox::{channel_provisioning::ChannelControlPlaneVerifier, CredentialError},
    store::{MongoBridgeAllocationStore, MongoSandboxCredentialStore},
};
use layer2_core::sandbox::Sandbox;
use std::sync::Arc;

#[async_trait::async_trait]
pub(crate) trait ChannelLifecycleCredentials: Send + Sync {
    async fn commit_phase(
        &self,
        sandbox: &Sandbox,
        mutation: &layer2_core::sandbox::store::SandboxLifecycleMutation,
    ) -> Result<(), CredentialError>;
    async fn require_restore(&self, sandbox: &Sandbox) -> Result<(), CredentialError>;
    async fn restore_credential(&self, sandbox: &Sandbox) -> Result<(), CredentialError>;
    async fn publish_active(&self, sandbox: &Sandbox) -> Result<(), CredentialError>;
}
pub(crate) struct ChannelLifecycleCredentialDriver {
    pub resources: Arc<crate::store::MongoSandboxStore>,
    pub authority: Arc<dyn ChannelRevivalAuthoritySource>,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub issuer: Arc<crate::auth::keys::KeyIssuer>,
    pub custody: Arc<MongoSandboxCredentialStore>,
    pub adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
}
#[async_trait::async_trait]
impl ChannelLifecycleCredentials for ChannelLifecycleCredentialDriver {
    async fn commit_phase(
        &self,
        sandbox: &Sandbox,
        mutation: &layer2_core::sandbox::store::SandboxLifecycleMutation,
    ) -> Result<(), CredentialError> {
        let permit = self.authority.resolve(sandbox).await.map_err(held)?;
        self.revalidate(&permit).await.map_err(held)?;
        crate::store::lifecycle_transaction::commit_channel_lifecycle(
            &self.resources,
            mutation.clone(),
            &self.allocations,
            &permit,
        )
        .await
        .map_err(held)
    }
    async fn require_restore(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        let permit = self.authority.resolve(sandbox).await.map_err(held)?;
        self.revalidate(&permit).await.map(|_| ()).map_err(held)
    }
    async fn restore_credential(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        self.run_credential(sandbox).await.map_err(held)
    }
    async fn publish_active(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        let permit = self.authority.resolve(sandbox).await.map_err(held)?;
        self.revalidate(&permit).await.map_err(held)?;
        self.allocations.change_channel_lifecycle(&permit, crate::store::bridge_allocations::channel_lifecycle::LifecycleCredentialChange::Publish).await.map(|_| ()).map_err(held)
    }
}
fn held(_error: impl std::fmt::Display) -> CredentialError {
    CredentialError::Unavailable("Channel revival requires the exact original allocation, lifecycle claim and renewable human authority".into())
}
