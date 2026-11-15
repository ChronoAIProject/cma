//! Durable admission around provider effects, independent of replica ownership.
use std::{future::Future, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{AdapterError, Result};

mod backend;
mod homes;
mod identity;
mod pools;
pub use backend::GuardedBackend;
pub use homes::GuardedHomeVolumes;

/// Only safe identifiers and a digest cross this persistence boundary. Provider
/// request bodies may contain credentials and must never become audit payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectIntent {
    pub operation: String,
    pub resource: String,
    pub request_sha256: String,
}

impl EffectIntent {
    pub fn new(operation: &str, resource: &str, request: &impl Serialize) -> Result<Self> {
        let intent = Self {
            operation: operation.into(),
            resource: resource.into(),
            request_sha256: crate::provider_request_digest(request)?,
        };
        intent.validate()?;
        Ok(intent)
    }

    pub fn validate(&self) -> Result<()> {
        if self.operation.is_empty()
            || self.operation.len() > 96
            || !self
                .operation
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
            || self.resource.is_empty()
            || self.resource.len() > 512
            || self.resource.chars().any(char::is_control)
            || !crate::provider_finality::digest(&self.request_sha256)
        {
            return Err(AdapterError::Rejected(
                "invalid provider effect identity".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedOutcome {
    Acknowledged,
    Unknown,
}

/// Acknowledgement is deliberately separate from settlement. The authority must
/// retain the permit until durable lifecycle/receiver/cleanup finality is proven.
#[async_trait::async_trait]
pub trait EffectPermit: Send + Sync {
    async fn record_outcome(&self, outcome: ObservedOutcome) -> Result<()>;
}

#[async_trait::async_trait]
pub trait EffectAuthority: Send + Sync {
    async fn admit(&self, intent: &EffectIntent) -> Result<Box<dyn EffectPermit>>;
}

pub(super) async fn dispatch<T>(
    authority: &Arc<dyn EffectAuthority>,
    intent: EffectIntent,
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    intent.validate()?;
    let permit = authority.admit(&intent).await?;
    // The provider future is not polled until durable admission succeeds. If
    // this future is cancelled, no Drop handler releases the durable permit.
    let result = operation.await;
    let outcome = if result.is_ok() {
        ObservedOutcome::Acknowledged
    } else {
        ObservedOutcome::Unknown
    };
    permit.record_outcome(outcome).await?;
    result
}

#[cfg(test)]
mod tests;
