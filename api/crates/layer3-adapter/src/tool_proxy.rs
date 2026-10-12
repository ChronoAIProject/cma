//! Correlated observation of the fixed, credential-injecting runtime endpoint.
use serde::{Deserialize, Serialize};

use crate::{AdapterError, Result};

pub const LOCAL_TOOL_PROXY_URL: &str = "http://127.0.0.1:9093/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolProxyRequest {
    pub sandbox_id: String,
    pub generation: u64,
    pub endpoint: String,
    pub credential_generation: u64,
}

impl ToolProxyRequest {
    pub fn validate(&self) -> Result<()> {
        let endpoint = url::Url::parse(&self.endpoint).map_err(|_| invalid())?;
        let loopback = endpoint.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if self.sandbox_id.is_empty()
            || self.sandbox_id.len() > 256
            || self.sandbox_id.trim() != self.sandbox_id
            || self.sandbox_id.chars().any(char::is_control)
            || self.generation == 0
            || self.credential_generation == 0
            || self.endpoint.len() > 2_048
            || endpoint.as_str() != self.endpoint
            || endpoint.host_str().is_none()
            || !(endpoint.scheme() == "https" || (endpoint.scheme() == "http" && loopback))
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.port() == Some(0)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolProxyStatus {
    pub request: ToolProxyRequest,
    pub boot_id: String,
}

impl ToolProxyStatus {
    pub fn validate_for(&self, request: &ToolProxyRequest) -> Result<()> {
        request.validate()?;
        if &self.request != request
            || self.boot_id.len() != 32
            || !self
                .boot_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("runtime tool endpoint observation is invalid".into())
}
