//! Scoped retirement stops only an entirely owned original Channel tree.
pub(crate) mod assessment;
mod driver;
pub(crate) mod model;
pub(crate) mod retention;
use crate::{
    bridge::provisioning::retirement::ChannelRetirementAuthoritySource,
    sandbox::{channel_provisioning::ChannelControlPlaneVerifier, CredentialError},
    store::{MongoBridgeAllocationStore, MongoSandboxCredentialStore},
};
use layer2_core::sandbox::Sandbox;
use std::{sync::Arc, time::SystemTime};

#[async_trait::async_trait]
pub(crate) trait ChannelRetirement: Send + Sync {
    async fn run(
        &self,
        sandbox: &Sandbox,
        agents: &dyn super::worker::LiveAgentStopper,
        now: SystemTime,
    ) -> Result<(), CredentialError>;
}
pub(crate) struct ChannelRetirementDriver {
    pub authority: Arc<dyn ChannelRetirementAuthoritySource>,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub issuer: Arc<crate::auth::keys::KeyIssuer>,
    pub custody: Arc<MongoSandboxCredentialStore>,
    pub adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
}
#[async_trait::async_trait]
impl ChannelRetirement for ChannelRetirementDriver {
    async fn run(
        &self,
        sandbox: &Sandbox,
        agents: &dyn super::worker::LiveAgentStopper,
        now: SystemTime,
    ) -> Result<(), CredentialError> {
        match self.run_step(sandbox, agents, now).await {
            Ok(driver::RetirementStep::Advanced) => Ok(()),
            Ok(driver::RetirementStep::Retained(assessment)) => {
                Err(CredentialError::ChannelRetentionHeld(assessment))
            }
            Err(_) => Err(held()),
        }
    }
}
fn held() -> CredentialError {
    CredentialError::Unavailable("Channel retirement is held until the original human authority, complete owned child tree and exact cleanup receipts are available; reused-parent cleanup and final retention cleanup require separate authority".into())
}
