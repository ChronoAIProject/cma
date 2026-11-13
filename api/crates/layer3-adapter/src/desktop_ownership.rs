//! Receiver-enforced authority for one Sandbox desktop stream.

use serde::{Deserialize, Serialize};

pub mod wire;

/// Concrete transport reached through the existing adapter/backend selection.
/// Absence is unsupported; it must never select a legacy stream as a fallback.
#[async_trait::async_trait]
pub trait DesktopOwnedRuntime: Send + Sync {
    async fn challenge(
        &self,
        handle: &crate::RuntimeHandle,
    ) -> crate::Result<DesktopOwnershipChallenge>;
    async fn open(
        &self,
        handle: &crate::RuntimeHandle,
        owner: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> crate::Result<crate::DesktopConnectionHandle>;
}

pub(crate) fn required(
    port: Option<&dyn DesktopOwnedRuntime>,
) -> crate::Result<&dyn DesktopOwnedRuntime> {
    port.ok_or_else(|| {
        crate::AdapterError::Unsupported("runtime desktop ownership is unavailable".into())
    })
}

pub(crate) async fn challenge(
    port: Option<&dyn DesktopOwnedRuntime>,
    handle: &crate::RuntimeHandle,
) -> crate::Result<DesktopOwnershipChallenge> {
    required(port)?.challenge(handle).await
}

pub(crate) async fn open(
    port: Option<&dyn DesktopOwnedRuntime>,
    handle: &crate::RuntimeHandle,
    owner: &DesktopOwnership,
    challenge: &DesktopOwnershipChallenge,
) -> crate::Result<crate::DesktopConnectionHandle> {
    required(port)?.open(handle, owner, challenge).await
}

/// Authenticated receiver control, independent of desktop frame backpressure.
#[async_trait::async_trait]
pub trait DesktopOwnershipControl: Send + Sync {
    /// Issue before Mongo renewal; the receiver owns the original deadline.
    async fn ownership_challenge(&self) -> crate::Result<DesktopOwnershipChallenge> {
        Err(crate::AdapterError::Unsupported(
            "desktop ownership challenge".into(),
        ))
    }

    /// Consume the challenge once and acknowledge its original deadline.
    async fn renew_ownership(
        &self,
        _: &DesktopOwnership,
        _: &DesktopOwnershipChallenge,
    ) -> crate::Result<()> {
        Err(crate::AdapterError::Unsupported(
            "desktop ownership renewal".into(),
        ))
    }

    /// Success proves exact-claim revocation and drained input at the receiver.
    /// Uncertainty must never authorize early durable claim release.
    async fn revoke_ownership(&self, _: &DesktopOwnership) -> crate::Result<()> {
        Err(crate::AdapterError::Unsupported(
            "desktop ownership revocation".into(),
        ))
    }
}

/// A durable claim, bound to the retained runtime and one connection attempt.
/// Times come from the shared store clock. Authority is installed only against
/// a receiver challenge issued before its durable acquisition/renewal. The
/// challenge's original monotonic deadline cannot be extended by replay.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopOwnership {
    pub sandbox_id: String,
    pub session_id: String,
    pub runtime_id: String,
    pub boot_id: Option<String>,
    pub epoch: u64,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}

/// Single-use nonce scoped by the receiver to its runtime boot and Sandbox.
/// The receiver starts its deadline before returning the challenge. A manager
/// must obtain a new challenge before every Mongo acquire/renew, then confirm
/// that challenge with the committed claim. Never reset its clock at confirm.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopOwnershipChallenge {
    pub nonce: String,
    pub valid_for_ms: u64,
}

impl DesktopOwnershipChallenge {
    pub fn validate(&self) -> crate::Result<()> {
        if self.nonce.trim().is_empty()
            || self.nonce.len() > 128
            || !(1..=10_000).contains(&self.valid_for_ms)
        {
            return Err(crate::AdapterError::Rejected(
                "invalid desktop ownership challenge".into(),
            ));
        }
        Ok(())
    }
}

impl DesktopOwnership {
    pub fn validate(&self) -> crate::Result<()> {
        let valid_id = |value: &str| !value.trim().is_empty() && value.len() <= 512;
        if !valid_id(&self.sandbox_id)
            || !valid_id(&self.runtime_id)
            || !valid_id(&self.session_id)
            || self.session_id.len() > 128
            || self.boot_id.as_ref().is_some_and(|value| !valid_id(value))
            || self.epoch == 0
            || self.epoch > i64::MAX as u64
            || self.issued_at_ms <= 0
            || !(1..=15_000).contains(&self.expires_at_ms.saturating_sub(self.issued_at_ms))
        {
            return Err(crate::AdapterError::Rejected(
                "invalid desktop ownership authority".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
