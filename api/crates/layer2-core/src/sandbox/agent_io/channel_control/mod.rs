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
mod refusal;
mod runtime;
mod validation;
pub use authority::{ChannelControlExecutionAuthorization, ChannelControlExecutionFence};
pub use refusal::ChannelControlRefusalSnapshot;

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

    /// Refusal only: prove a definite local change to the retained source or
    /// consent and atomically settle the unchanged hold. Missing or uncertain
    /// evidence cannot authorize either execution or a terminal refusal.
    async fn refuse_control_hold(
        &self,
        command: &AdmittedAgentCommand,
        deadline: tokio::time::Instant,
    ) -> Result<(), Error>;
}

pub struct ChannelControlExecutionService {
    store: Arc<dyn ChannelControlExecutionStore>,
    invokes: BridgeInvokeService,
    feature_gates: Option<Arc<dyn crate::bridge::BridgeConnectionValidator>>,
}
impl ChannelControlExecutionService {
    pub fn new(store: Arc<dyn ChannelControlExecutionStore>, invokes: BridgeInvokeService) -> Self {
        Self {
            store,
            invokes,
            feature_gates: None,
        }
    }
    /// Deployment availability gates only held work entering the shared FIFO.
    /// Released work and outcome reconciliation keep their current authority.
    pub fn with_feature_gates(
        mut self,
        gates: Arc<dyn crate::bridge::BridgeConnectionValidator>,
    ) -> Self {
        self.feature_gates = Some(gates);
        self
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
    /// Check the same local runtime registry used for dispatch before releasing
    /// the hold. Local absence preserves the hold and does not revoke authority.
    pub async fn release_held(
        &self,
        command: &AdmittedAgentCommand,
        operations: &crate::sandbox::AgentOperationService,
    ) -> Result<(), Error> {
        if command.queue_expired(&crate::now_rfc3339()) {
            return self.store.expire_control_hold(command).await;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let authority = match ChannelControlExecutionAuthorization::observe(
            self.store.clone(),
            self.invokes.clone(),
            command,
            deadline,
            None,
        )
        .await
        {
            Ok(authority) => authority,
            Err(Error::SourceUnavailable | Error::AuthorityUnavailable) => {
                return tokio::time::timeout_at(
                    deadline,
                    self.store.refuse_control_hold(command, deadline),
                )
                .await
                .map_err(|_| Error::Unavailable)?;
            }
            Err(error) => return Err(error),
        };
        let fence = authority.fence();
        fence.require_held(command)?;
        if let Some(gates) = &self.feature_gates {
            gates
                .check_feature(
                    crate::bridge::BridgeFeature::Execution,
                    &fence.proof().grant().config.connector.platform,
                )
                .map_err(|_| Error::Unavailable)?;
        }
        runtime::require_local(
            operations,
            fence.proof(),
            fence.snapshot(),
            fence.deadline(),
        )
        .await?;
        tokio::time::timeout_at(
            fence.deadline(),
            self.store.release_control_hold(fence, command),
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
