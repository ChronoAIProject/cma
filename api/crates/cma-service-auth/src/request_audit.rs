//! Bounded Gateway-to-Manager HTTP audit protocol.
use serde::{Deserialize, Serialize};

pub const PATH: &str = "/api/v1/cmaeg/request-audit";
pub const WRITE_PERMISSION: &str = "cma:gateway:audit:write";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Reserve {
        request_id: String,
        method: String,
        route: String,
    },
    Bind {
        request_id: String,
        identity: Identity,
    },
    Finish {
        request_id: String,
        status: u16,
    },
}

impl Event {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Reserve { request_id, .. }
            | Self::Bind { request_id, .. }
            | Self::Finish { request_id, .. } => request_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub subject: String,
    pub scope: Scope,
    pub organization: Option<String>,
    pub credential: Credential,
    pub client: Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Personal,
    Organization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Credential {
    WebSession,
    Bearer,
    OwnerKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    Web,
    Api,
    Cli,
    Sdk,
}
