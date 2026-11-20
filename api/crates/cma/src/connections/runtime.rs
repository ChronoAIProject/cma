use std::sync::Arc;

use layer2_core::{
    profiles::ModelAccess,
    sandbox::{
        store::{AgentStore, SandboxStore},
        SandboxStatus, WarmSandboxState,
    },
    AgentId, SandboxId,
};
use layer3_adapter::{
    hosted::{HostedSubscriptionCredential, HostedSubscriptionSource},
    AdapterError, AgentSlot, RuntimeCredential, RuntimeHandle,
};

use crate::store::MongoSandboxStore;

use super::CodexSubscriptions;

pub(crate) struct RuntimeSubscriptions {
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    store: Arc<MongoSandboxStore>,
    subscriptions: Arc<CodexSubscriptions>,
}

impl RuntimeSubscriptions {
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        subscriptions: Arc<CodexSubscriptions>,
    ) -> Arc<Self> {
        Arc::new(Self {
            standing: Arc::new(layer2_core::account_deletion::AvailableAccounts),
            store,
            subscriptions,
        })
    }
    pub(crate) fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("subscription standing configured before sharing")
            .standing = standing;
        self
    }
}

#[async_trait::async_trait]
impl HostedSubscriptionSource for RuntimeSubscriptions {
    async fn credential(
        &self,
        sandbox_id: &str,
        runtime: &RuntimeHandle,
        agent: &AgentSlot,
        force_refresh: bool,
        previous_account: Option<&str>,
    ) -> layer3_adapter::Result<HostedSubscriptionCredential> {
        let id = SandboxId(sandbox_id.to_owned());
        if let Some(sandbox) = self
            .store
            .get_sandbox(&id)
            .await
            .map_err(|_| unavailable())?
        {
            if self
                .standing
                .authoritative(&sandbox.owner)
                .await
                .map_err(|_| unavailable())?
            {
                return Err(AdapterError::AccessRevoked);
            }
            if sandbox.profile.content.model_access != ModelAccess::OwnCredentials
                || matches!(
                    sandbox.status,
                    SandboxStatus::Paused
                        | SandboxStatus::Failed
                        | SandboxStatus::Retiring
                        | SandboxStatus::Retired
                )
                || sandbox
                    .runtime
                    .as_ref()
                    .is_none_or(|binding| binding.runtime_id != runtime.as_str())
            {
                return Err(refused());
            }
            let agent = self
                .store
                .get_agent(&AgentId(agent.0.clone()))
                .await
                .map_err(|_| unavailable())?
                .filter(|agent| {
                    agent.sandbox_id == id && agent.owner == sandbox.owner && agent.status.is_live()
                })
                .ok_or_else(refused)?;
            let tokens = self
                .subscriptions
                .tokens(&agent.owner, force_refresh, previous_account)
                .await
                .map_err(|_| refused())?;
            return Ok(HostedSubscriptionCredential::Connected {
                access_token: RuntimeCredential::new(tokens.access_token.expose()),
                account_id: tokens.account_id,
                plan_type: tokens.plan_type,
            });
        }
        let warm = self
            .store
            .warm_sandbox(&id)
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(refused)?;
        if force_refresh
            || previous_account.is_some()
            || warm.plan.profile.content.model_access != ModelAccess::OwnCredentials
            || !matches!(
                warm.state,
                WarmSandboxState::Preparing
                    | WarmSandboxState::Installing
                    | WarmSandboxState::Ready
            )
            || warm
                .runtime
                .as_ref()
                .is_none_or(|binding| binding.runtime_id != runtime.as_str())
            || !warm
                .workspaces
                .iter()
                .flat_map(|workspace| &workspace.agents)
                .any(|candidate| candidate.id.as_str() == agent.as_str())
        {
            return Err(refused());
        }
        Ok(HostedSubscriptionCredential::Warm)
    }
}

fn refused() -> AdapterError {
    AdapterError::Rejected("Re-login with Codex subscription to continue".into())
}

fn unavailable() -> AdapterError {
    AdapterError::Unavailable("subscription ownership could not be verified".into())
}
