pub use layer3_adapter::{BrowserFrame, BrowserObservation, BrowserState};
use serde::Serialize;

pub struct BrowserSelection {
    pub runtime_incarnation: String,
    pub observer: Option<String>,
    pub context_id: Option<String>,
    pub enabled: Option<bool>,
    pub release: bool,
}

#[derive(Debug, Serialize)]
pub struct BrowserObservationView {
    pub agent_id: String,
    pub runtime_incarnation: String,
    #[serde(flatten)]
    pub observation: BrowserObservation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentAvailability {
    Available,
    Unsupported,
    NotEnabled,
    Forbidden,
    TemporarilyUnavailable,
}

#[derive(Debug, Serialize)]
pub struct EnvironmentFeature {
    pub state: EnvironmentAvailability,
    pub reason: &'static str,
    pub scope: &'static str,
    pub writable: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentEnvironment {
    pub agent_id: String,
    pub policy_revision: u64,
    pub runtime_incarnation: Option<String>,
    pub revalidate_at: Option<String>,
    pub files: EnvironmentFeature,
    pub operation_output: EnvironmentFeature,
    pub browser: EnvironmentFeature,
    pub replay: EnvironmentFeature,
    pub control: EnvironmentFeature,
}

impl EnvironmentFeature {
    pub(super) fn unsupported() -> Self {
        Self {
            state: EnvironmentAvailability::Unsupported,
            reason: "not_supported_by_contract",
            scope: "none",
            writable: false,
        }
    }
}
