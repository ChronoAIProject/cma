#![doc = "Read-only Agent computer capability and scope observations."]
use serde::Deserialize;

#[doc = "Availability under the currently authorized public contract."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAvailability {
    #[doc = "The operation is supported and currently authorized."]
    Available,
    #[doc = "This contract does not support the operation."]
    Unsupported,
    #[doc = "The runtime is not enabled for the operation."]
    NotEnabled,
    #[doc = "The current caller lacks the necessary permission."]
    Forbidden,
    #[doc = "The runtime is temporarily unable to serve the operation."]
    TemporarilyUnavailable,
}

#[doc = "One feature's availability and explicit data scope; never a retained grant."]
#[derive(Debug, Clone, Deserialize)]
pub struct EnvironmentFeature {
    #[doc = "Current availability."]
    pub state: EnvironmentAvailability,
    #[doc = "Stable reason code."]
    pub reason: String,
    #[doc = "Mounted home, Agent turn history, or no scope."]
    pub scope: String,
    #[doc = "Whether mutations are currently authorized; each mutation rechecks permission."]
    pub writable: bool,
}

#[doc = "Environment observation scoped to one authorized Agent."]
#[derive(Debug, Clone, Deserialize)]
pub struct AgentEnvironment {
    #[doc = "Exact Agent identity."]
    pub agent_id: String,
    #[doc = "Policy revision under which this observation was made."]
    pub policy_revision: u64,
    #[doc = "Opaque runtime incarnation, without provider addresses or boot identifiers."]
    pub runtime_incarnation: Option<String>,
    #[doc = "Authorization renewal deadline when supplied by policy."]
    pub revalidate_at: Option<String>,
    #[doc = "Authorized mounted-home file operations."]
    pub files: EnvironmentFeature,
    #[doc = "Read-only recorded Agent turn output."]
    pub operation_output: EnvironmentFeature,
    #[doc = "Browser observation capability."]
    pub browser: EnvironmentFeature,
    #[doc = "Recorded environment replay capability."]
    pub replay: EnvironmentFeature,
    #[doc = "Interactive control capability."]
    pub control: EnvironmentFeature,
}
