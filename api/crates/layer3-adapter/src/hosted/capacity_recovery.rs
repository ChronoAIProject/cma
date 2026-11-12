//! This path accepts only a dedicated native proof producer. There is no fallback
//! to ordinary open/reconnect, context/settings replay or runtime replacement.
use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::agent_capacity_recovery::{
    CapacityRecoveryConfirmation, CapacityRecoveryConnection, CapacityRecoveryError,
    CapacityRecoveryRequest, CapacityRecoveryResult,
};
use crate::{ConversationChannel, ConversationUpdate};

/// Implemented by a backend that can fence and inspect the retained native
/// process, attach it without input, and bind its output boundary atomically.
/// Unsupported retained runtimes must refuse; no state may be inferred from the
/// caller's expected active turn, a successful socket attach or an open receipt.
#[async_trait]
pub trait CapacityRecoveryNativeTransport: Send + Sync {
    async fn reattach_existing(
        &self,
        request: &CapacityRecoveryRequest,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> CapacityRecoveryResult<(ConversationChannel, CapacityRecoveryConfirmation)>;
}

pub(super) async fn reattach(
    transport: Option<&dyn CapacityRecoveryNativeTransport>,
    request: &CapacityRecoveryRequest,
    updates: mpsc::Sender<ConversationUpdate>,
) -> CapacityRecoveryResult<CapacityRecoveryConnection> {
    request.validate()?;
    let transport = transport.ok_or(CapacityRecoveryError::Unsupported)?;
    let (channel, confirmation) = transport.reattach_existing(request, updates).await?;
    CapacityRecoveryConnection::verified(request, channel, confirmation)
}

#[cfg(test)]
mod tests;
