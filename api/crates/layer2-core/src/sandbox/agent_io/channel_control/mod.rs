//! Fresh exact-source control execution. Retained intent and references are
//! data; only this service can construct a nonrenewing execution authorization.
use std::{sync::Arc, time::Duration};

use super::{AdmittedAgentCommand, ChannelCommandExecutionSource, CommandLane};
use crate::bridge::{
    channel_control::{AdmittedChannelControl, ChannelControlError as Error},
    invoke::{BridgeInvokeProof, BridgeInvokeService, SourceControlPolicy, SourceControlReceipt},
};

mod authority;
mod claimed;
pub(in crate::sandbox) mod dispatch;
mod readiness;
mod validation;
pub use authority::{ChannelControlExecutionAuthorization, ChannelControlExecutionFence};

/// One store snapshot, never authority. The adapter checks permanent intent,
/// reference and source records and complete current consent receipt history.
#[derive(Clone)]
pub struct ChannelControlExecutionSnapshot {
    pub admitted: AdmittedChannelControl,
    pub source_command: AdmittedAgentCommand,
    pub source: ChannelCommandExecutionSource,
    pub consent: SourceControlPolicy,
    pub consent_receipt: SourceControlReceipt,
    pub lane: Option<CommandLane>,
}

#[async_trait::async_trait]
pub trait ChannelControlExecutionStore: Send + Sync {
    async fn read_control_execution(
        &self,
        command: &AdmittedAgentCommand,
        deadline: tokio::time::Instant,
    ) -> Result<ChannelControlExecutionSnapshot, Error>;

    async fn release_control_hold(
        &self,
        fence: Arc<ChannelControlExecutionFence>,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error>;

    /// This port can only record a proved expiration, never grant execution.
    async fn expire_control_hold(&self, command: &AdmittedAgentCommand) -> Result<(), Error>;
}

pub struct ChannelControlExecutionService {
    store: Arc<dyn ChannelControlExecutionStore>,
    invokes: BridgeInvokeService,
}
impl ChannelControlExecutionService {
    pub fn new(store: Arc<dyn ChannelControlExecutionStore>, invokes: BridgeInvokeService) -> Self {
        Self { store, invokes }
    }
    pub async fn authorize(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<ChannelControlExecutionAuthorization, Error> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        ChannelControlExecutionAuthorization::observe(
            self.store.clone(),
            self.invokes.clone(),
            command,
            deadline,
            None,
        )
        .await
    }
    pub async fn release_held(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        if command.queue_expired(&crate::now_rfc3339()) {
            return self.store.expire_control_hold(command).await;
        }
        let authority = self.authorize(command).await?;
        authority.fence().require_held(command)?;
        tokio::time::timeout_at(
            authority.fence().deadline(),
            self.store.release_control_hold(authority.fence(), command),
        )
        .await
        .map_err(|_| Error::Unavailable)?
    }
}

fn invoke_error(error: crate::bridge::invoke::BridgeInvokeError) -> Error {
    use crate::bridge::invoke::BridgeInvokeError as Invoke;
    match error {
        Invoke::Unavailable => Error::Unavailable,
        Invoke::NotFound | Invoke::TargetMismatch => Error::SourceUnavailable,
        _ => Error::AuthorityUnavailable,
    }
}
