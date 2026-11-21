//! Persisted Pod capabilities keep retained pool templates and digests immutable.
use super::{AgentSandboxConfig, NativePoolSpec, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeFsGroupChangePolicy {
    OnRootMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePoolPodPolicy {
    pub runtime_class: Option<String>,
    pub service_account: String,
    pub image_pull_secrets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_network_policy: Option<super::super::NativeTemplateNetworkPolicy>,
    // Missing means the historical Kubernetes default, including its original
    // template bytes and pool identity. Never fill this from current defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_group_change_policy: Option<NativeFsGroupChangePolicy>,
}

impl From<&AgentSandboxConfig> for NativePoolPodPolicy {
    fn from(config: &AgentSandboxConfig) -> Self {
        Self {
            runtime_class: config.runtime_class.clone(),
            service_account: config.service_account.clone(),
            image_pull_secrets: config.image_pull_secrets.clone(),
            template_network_policy: config.template_network_policy.clone(),
            fs_group_change_policy: config.template_fs_group_change_policy,
        }
    }
}

pub(super) fn policy_digest(
    namespace: &str,
    deployment: &str,
    policy: &NativePoolPodPolicy,
    spec: &NativePoolSpec,
) -> Result<String> {
    let original = (
        "native-pool-v1",
        namespace,
        deployment,
        &policy.runtime_class,
        &policy.service_account,
        &policy.image_pull_secrets,
        spec,
    );
    match (
        &policy.template_network_policy,
        policy.fs_group_change_policy,
    ) {
        (None, None) => crate::provider_request_digest(&original),
        (Some(network), None) => {
            crate::provider_request_digest(&("native-pool-v2", original, network))
        }
        (network, Some(fs_group)) => {
            crate::provider_request_digest(&("native-pool-v3", original, network, fs_group))
        }
    }
}
