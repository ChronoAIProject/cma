//! Provider evidence for recovery when no runtime container can accept commands.
//! This is an observation only. The lifecycle owner must prove there are no Agents
//! and revalidate this exact observation immediately before suspension.
use serde::{Deserialize, Serialize};

use crate::RuntimeHandle;

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NonexecutingRuntimeProof {
    pub(crate) version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) idle_bootstrap: Option<IdleBootstrapProof>,
    pub(crate) handle: RuntimeHandle,
    pub(crate) request_sha256: String,
    pub(crate) home_identity: String,
    pub(crate) sandbox_generation: i64,
    pub(crate) sandbox_resource_version: String,
    pub(crate) pod_name: String,
    pub(crate) pod_uid: String,
    pub(crate) pod_resource_version: String,
    pub(crate) runtime_container_id: String,
    pub(crate) runtime_restart_count: i32,
    pub(crate) state_sha256: String,
}

/// Full process/descriptor proof from the specifically supported legacy bootstrap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IdleBootstrapProof {
    pub version: u32,
    pub pid_start_ticks: u64,
    pub binary_sha256: String,
    pub process_sha256: String,
}
