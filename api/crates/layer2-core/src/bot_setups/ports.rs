use super::*;
use crate::{bots::BotError, resource_access::ResourceOwner};

#[async_trait::async_trait]
pub trait BotSetupStore: Send + Sync {
    async fn get(&self, actor: &ExternalActor) -> Result<Option<BotSetup>, BotError>;
    async fn insert(&self, setup: &BotSetup) -> Result<(), BotError>;
    async fn replace(&self, setup: &BotSetup, expected_revision: u64) -> Result<(), BotError>;
}

#[async_trait::async_trait]
pub trait BotSetupDependencies: Send + Sync {
    /// Attest sender and input using the gateway's sealed, integrity-checked event.
    async fn event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
    ) -> Result<SetupEvent, BotError>;
    async fn identity(&self, actor: &ExternalActor) -> Result<SetupIdentity, BotError>;
    async fn confirm_identity(
        &self,
        actor: &ExternalActor,
        proof: &str,
    ) -> Result<SetupIdentity, BotError>;
    async fn owners(
        &self,
        actor: &ExternalActor,
        grant: &str,
    ) -> Result<Vec<SetupOwnerChoice>, BotError>;
    async fn defaults(&self) -> Result<SetupConfiguration, BotError>;
    async fn validate_configuration(
        &self,
        configuration: &SetupConfiguration,
    ) -> Result<(), BotError>;
    async fn services(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
    ) -> Result<Vec<SetupServiceChoice>, BotError>;
    async fn connect(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
        service: &str,
    ) -> Result<String, BotError>;
    async fn external_bots(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
    ) -> Result<Vec<SetupExternalBot>, BotError>;
    async fn external_bot_registration_url(
        &self,
        owner: &ResourceOwner,
    ) -> Result<String, BotError>;
    /// Must recheck current grant, membership, services and the immutable plan.
    /// Replays use the setup ID as a durable, owner-scoped idempotency key.
    async fn provision(&self, setup: &BotSetup) -> Result<SetupResult, BotError>;
    async fn result(&self, setup: &BotSetup) -> Result<SetupResult, BotError>;
}
