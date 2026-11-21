pub use layer3_adapter::{
    BrowserControlAudit, BrowserControlCommand, BrowserControlState, BrowserControlStatus,
    BrowserInput,
};
use serde::Serialize;

pub struct BrowserControlSelection {
    pub runtime_incarnation: String,
    pub context_id: String,
    pub client_id: String,
    pub client_key: Option<String>,
    pub command: BrowserControlCommand,
    pub operation_id: Option<String>,
    pub expected_revision: Option<u64>,
    pub lease_token: Option<String>,
    pub input: Option<BrowserInput>,
}

#[derive(Debug, Serialize)]
pub struct BrowserControlView {
    pub agent_id: String,
    pub runtime_incarnation: String,
    pub context_id: String,
    #[serde(flatten)]
    pub control: BrowserControlStatus,
}
