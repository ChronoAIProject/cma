//! Internal Agent Manager ownership negotiation, independent of public APIs.
use serde::{Deserialize, Serialize};
use std::{future::Future, sync::Arc};

#[async_trait::async_trait]
pub trait ReceiverRevocation: Send + Sync {
    fn identity(&self) -> String;
    async fn revoke(&self) -> crate::Result<()>;
}

#[async_trait::async_trait]
pub trait TransportAuthority: Send + Sync {
    fn owner(&self) -> TransportOwner;
    fn is_current(&self) -> bool;
    fn register_receiver(&self, receiver: Arc<dyn ReceiverRevocation>) -> crate::Result<()>;
    async fn check(&self) -> crate::Result<()>;
    /// Must renew the exact durable owner only after receiving a fresh runtime
    /// challenge. Never confirm a challenge when this call fails or is cancelled.
    async fn renew_for_challenge(&self) -> crate::Result<()>;
    async fn revoked(&self);
    fn revoke(&self);
}

tokio::task_local! {
    static AUTHORITY: Arc<dyn TransportAuthority>;
}

pub fn current_authority() -> Option<Arc<dyn TransportAuthority>> {
    AUTHORITY.try_with(Arc::clone).ok()
}

pub async fn scope<F: Future>(authority: Arc<dyn TransportAuthority>, operation: F) -> F::Output {
    AUTHORITY.scope(authority, operation).await
}

pub fn current_header() -> crate::Result<Option<String>> {
    current_authority()
        .map(|authority| authority.owner().header())
        .transpose()
}

pub const OWNER_PROTOCOL_VERSION: u32 = 1;
pub const OWNER_HEADER: &str = "x-cma-manager-ownership";
/// Starts at the receiver before the manager's subsequent Mongo renewal.
pub const RECEIVER_LEASE_MS: u64 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportOwner {
    pub protocol: u32,
    pub agent_id: String,
    pub instance_id: String,
    pub epoch: u64,
}

impl TransportOwner {
    pub fn validate(&self) -> crate::Result<()> {
        if self.protocol != OWNER_PROTOCOL_VERSION
            || self.epoch == 0
            || self.agent_id.is_empty()
            || self.agent_id.len() > 128
            || self.instance_id.is_empty()
            || self.instance_id.len() > 128
        {
            return Err(crate::AdapterError::Rejected(
                "invalid transport ownership token".into(),
            ));
        }
        Ok(())
    }

    pub fn header(&self) -> crate::Result<String> {
        use base64::Engine;
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|_| crate::AdapterError::Rejected("ownership token encoding failed".into()))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn from_header(value: &str) -> crate::Result<Self> {
        use base64::Engine;
        let invalid = || crate::AdapterError::Rejected("invalid transport ownership header".into());
        if value.len() > 1_024 {
            return Err(invalid());
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| invalid())?;
        let token: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        token.validate()?;
        Ok(token)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerChallenge {
    pub protocol: u32,
    pub nonce: String,
    pub valid_for_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerConfirmation {
    pub owner: TransportOwner,
    pub nonce: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerAcknowledgement {
    pub owner: TransportOwner,
    pub protocol: u32,
}

#[cfg(test)]
#[path = "owner_protocol_test_support.rs"]
pub(crate) mod test_support;
