//! Injectable authority ports shared by setup, delegated calls and continuations.
use super::*;
use crate::sandbox::SandboxCredentialCustody;
use layer2_core::{
    sandbox::{agent_io::AgentCommandStore, binding::BindingStore},
    ActivityStore,
};
use serde_json::Value;

pub(super) trait EventStore: BindingStore + AgentCommandStore {}
impl<T: BindingStore + AgentCommandStore> EventStore for T {}

#[async_trait::async_trait]
pub(super) trait ContextStore: Send + Sync {
    async fn retain(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
        expires_at: i64,
        context: &Value,
    ) -> Result<(), BotError>;
    async fn get(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<Option<Value>, BotError>;
}

#[async_trait::async_trait]
impl ContextStore for crate::store::bot_setup_contexts::MongoBotSetupContextStore {
    async fn retain(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
        expires_at: i64,
        context: &Value,
    ) -> Result<(), BotError> {
        self.retain(binding, conversation, event, expires_at, context)
            .await
    }
    async fn get(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<Option<Value>, BotError> {
        self.get(binding, conversation, event).await
    }
}

pub(crate) struct SetupAuthority {
    pub(super) auth: Arc<AuthState>,
    pub(super) store: Arc<dyn EventStore>,
    pub(super) custody: Arc<dyn SandboxCredentialCustody>,
    pub(super) activity: Arc<dyn ActivityStore>,
    pub(super) contexts: Arc<dyn ContextStore>,
    pub(super) http: reqwest::Client,
    pub(super) gateway: String,
    pub(super) binding: String,
}

impl SetupAuthority {
    pub(super) async fn json(&self, request: reqwest::RequestBuilder) -> Result<Value, BotError> {
        request_json(request).await
    }
    pub(super) async fn link_identity(
        &self,
        value: &ExternalActor,
    ) -> Result<SetupIdentity, BotError> {
        let status = self
            .auth
            .ensure_actor_link(&super::authorization::actor(value))
            .await?;
        Ok(SetupIdentity {
            grant_id: status.grant_id,
            login_url: status.login_url,
            subject: status.candidate.as_ref().map(|c| c.subject.to_string()),
            display_name: status.candidate.and_then(|c| c.profile.display_name),
        })
    }
}
