//! Durable targets and observed readiness for native runtime warm pools.

use super::Administration;
use crate::{Error, IdempotencyKey, Result, resources::identity};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

string_enum! { /// Reconciliation state of a durable native pool target.
    pub enum WarmPoolStatus {
        Pending => "pending", Ready => "ready", Disabled => "disabled", Unavailable => "unavailable"
    }
}

/// A durable pool target, retaining its originally selected runtime image.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WarmPoolView {
    /// Stable pool target identity.
    pub id: String,
    /// Current revision required by a resize command.
    pub revision: u64,
    /// Immutable runtime image selected when this target was created.
    pub image: String,
    /// Immutable image, resources, home and Pod policy identity.
    pub config_sha256: String,
    /// CPU reservation for each member, in millicores.
    pub cpu_millis: u32,
    /// Memory reservation for each member, in MiB.
    pub memory_mib: u32,
    /// Requested count of unclaimed members.
    pub target_replicas: u32,
    /// Observed ready members; `None` means readiness is unknown.
    pub ready_replicas: Option<u32>,
    /// Whether the controller has observed the requested configuration.
    pub observed: bool,
    /// Current reconciliation state.
    pub status: WarmPoolStatus,
    /// Original provider resource name, if bound.
    pub resource_name: Option<String>,
    /// Safe reconciliation failure detail, if available.
    pub error: Option<String>,
}

/// Pool inventory and deployment defaults for creating a new target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WarmPoolList {
    /// Recorded pool targets, including targets whose provider is unavailable.
    pub items: Vec<WarmPoolView>,
    /// Trusted runtime image selected for newly created targets.
    pub default_image: String,
    /// Default member CPU reservation, in millicores.
    pub default_cpu_millis: u32,
    /// Default member memory reservation, in MiB.
    pub default_memory_mib: u32,
    /// Exact default configuration; `None` when the server cannot plan it.
    pub default_config_sha256: Option<String>,
    /// Whether provider inventory could be observed.
    pub available: bool,
}

/// Create a pool using the server's trusted image and home policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureWarmPoolBody {
    /// Member CPU reservation, in millicores; 1 through 1,048,576.
    pub cpu_millis: u32,
    /// Member memory reservation, in MiB; 1 through 1,048,576.
    pub memory_mib: u32,
    /// Requested unclaimed capacity; 0 through 1,000.
    pub target_replicas: u32,
}

/// Change only the replica target of an existing original pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResizeWarmPoolBody {
    /// Current revision read from inventory; positive and below `i64::MAX`.
    pub expected_revision: u64,
    /// Requested unclaimed capacity; zero stops replenishment.
    pub target_replicas: u32,
}

impl Administration<'_> {
    /// List durable native pool targets and observed provider readiness.
    pub async fn list_warm_pools(&self) -> Result<WarmPoolList> {
        self.client.get(&["admin", "warm-pools"], &[]).await
    }

    /// Admit configuration with a stable key of at most 128 visible ASCII bytes.
    /// A 202 response records intent; poll inventory for reconciliation progress.
    pub async fn configure_warm_pool(
        &self,
        body: &ConfigureWarmPoolBody,
        key: &IdempotencyKey,
    ) -> Result<WarmPoolView> {
        target(body.target_replicas)?;
        if !(1..=1_048_576).contains(&body.cpu_millis)
            || !(1..=1_048_576).contains(&body.memory_mib)
            || key.as_str().len() > 128
        {
            return Err(Error::validation(
                "pool CPU/memory must be 1 to 1048576 and the idempotency key at most 128 bytes",
            ));
        }
        self.client
            .command(
                Method::POST,
                &["admin", "warm-pools"],
                Some(body),
                key,
                StatusCode::ACCEPTED,
            )
            .await
    }

    /// Resize an original pool at its current revision. Zero stops replenishment
    /// while claimed Agent runtimes keep their existing homes and lifecycle.
    pub async fn resize_warm_pool(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
    ) -> Result<WarmPoolView> {
        target(body.target_replicas)?;
        if body.expected_revision == 0 || body.expected_revision >= i64::MAX as u64 {
            return Err(Error::validation(
                "expected_revision must be positive and below i64::MAX",
            ));
        }
        let result: WarmPoolView = self
            .client
            .operation(
                Method::PUT,
                &["admin", "warm-pools", id],
                Some(body),
                StatusCode::ACCEPTED,
            )
            .await?;
        identity(&result.id, id)?;
        Ok(result)
    }
}

fn target(replicas: u32) -> Result<()> {
    if replicas > 1000 {
        return Err(Error::validation("target_replicas must be 0 to 1000"));
    }
    Ok(())
}
