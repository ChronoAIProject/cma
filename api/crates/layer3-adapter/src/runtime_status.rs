//! Observe one verified persistent runtime without disclosing credentials or file contents.

use crate::{AdapterError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatusRequest {
    pub sandbox_id: String,
    pub generation: u64,
    pub home_identity: String,
}

impl RuntimeStatusRequest {
    pub fn validate(&self) -> Result<()> {
        if self.sandbox_id.is_empty()
            || self.sandbox_id.len() > 256
            || self.sandbox_id.trim() != self.sandbox_id
            || self.sandbox_id.chars().any(char::is_control)
            || self.generation == 0
            || !valid_home_identity(&self.home_identity)
        {
            return Err(AdapterError::Rejected(
                "runtime status identity is invalid".into(),
            ));
        }
        Ok(())
    }
}

/// Pins the Supervisor that proves the predecessor process or accepts its successor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRestartRuntime {
    pub runtime: RuntimeStatusRequest,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStatus {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_browser: bool,
    /// Runtime supports descriptor-confined management of the complete persistent home.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_filesystem: bool,
    /// Exact owner uploads can be published under the complete persistent home.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_attachment_upload: bool,
    /// Absent on retained supervisors that predate the additive operation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_skill_install: bool,
    /// Fenced replacement of a proved previous installation. Negotiated on status reads.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_skill_upgrade: bool,
    /// Managed model selection can resolve current catalogue aliases.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_model_catalog: bool,
    /// One ordered request can acknowledge both fenced Agent resource operations.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_resource_preparation: bool,
    /// This runtime accepts a new process for an Agent with a proved Close.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent_process_restart: bool,
    /// The identified supervisor can install a generation-bound GitHub route.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exact_github_credential_route: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub github_repository_scope: bool,
    pub sandbox_id: String,
    pub generation: u64,
    pub home_identity: String,
    /// Changes on every supervisor process start, including a replacement behind the same handle.
    pub boot_id: String,
    pub installation_ready: bool,
    pub credential_ready: bool,
    pub restore_pending: bool,
    pub agent_processes: u16,
}

impl RuntimeStatus {
    pub fn restart_runtime(&self) -> Result<AgentRestartRuntime> {
        if !self.agent_process_restart {
            return Err(AdapterError::Unsupported(
                "agent_process_restart_unsupported".into(),
            ));
        }
        self.ready_process_identity()
    }

    /// Identify the exact ready Supervisor for a proof of an existing process.
    /// Reading a Close proof does not authorize a successor process.
    pub fn ready_process_identity(&self) -> Result<AgentRestartRuntime> {
        let runtime = RuntimeStatusRequest {
            sandbox_id: self.sandbox_id.clone(),
            generation: self.generation,
            home_identity: self.home_identity.clone(),
        };
        self.validate_for(&runtime)?;
        if !self.installation_ready || !self.credential_ready || self.restore_pending {
            return Err(AdapterError::Unavailable(
                "agent_process_restart_runtime_unready".into(),
            ));
        }
        Ok(AgentRestartRuntime {
            runtime,
            boot_id: self.boot_id.clone(),
        })
    }

    pub fn validate_for(&self, request: &RuntimeStatusRequest) -> Result<()> {
        request.validate()?;
        if self.sandbox_id != request.sandbox_id
            || self.generation != request.generation
            || self.home_identity != request.home_identity
            || self.boot_id.len() != 32
            || !self
                .boot_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || (self.installation_ready && self.restore_pending)
            || (self.credential_ready && !self.installation_ready && !self.restore_pending)
            || (self.agent_processes != 0 && !self.installation_ready)
            || self.agent_processes > 16
        {
            return Err(AdapterError::Rejected(
                "runtime status does not prove the requested identity".into(),
            ));
        }
        Ok(())
    }
}

pub fn valid_home_identity(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn restart_runtime_requires_ready_negotiated_capability() {
        let requested = RuntimeStatusRequest {
            sandbox_id: "sandbox".into(),
            generation: 7,
            home_identity: "a".repeat(43),
        };
        let mut status: RuntimeStatus = serde_json::from_value(serde_json::json!({
            "sandbox_id": "sandbox", "generation": 7,
            "home_identity": "a".repeat(43), "boot_id": "b".repeat(32),
            "installation_ready": true, "credential_ready": false,
            "restore_pending": false, "agent_processes": 0
        }))
        .unwrap();
        assert!(matches!(
            status.restart_runtime(),
            Err(AdapterError::Unsupported(_))
        ));
        assert!(matches!(
            status.ready_process_identity(),
            Err(AdapterError::Unavailable(_))
        ));
        status.credential_ready = true;
        let identity = AgentRestartRuntime {
            runtime: requested,
            boot_id: "b".repeat(32),
        };
        assert_eq!(status.ready_process_identity().unwrap(), identity);
        assert!(matches!(
            status.restart_runtime(),
            Err(AdapterError::Unsupported(_))
        ));
        status.agent_process_restart = true;
        assert_eq!(status.restart_runtime().unwrap(), identity);
    }

    #[test]
    fn retained_runtime_without_catalogue_capability_stays_readable() {
        let original = serde_json::json!({
            "sandbox_id": "sandbox", "generation": 1,
            "home_identity": "a".repeat(43), "boot_id": "a".repeat(32),
            "installation_ready": true, "credential_ready": true,
            "restore_pending": false, "agent_processes": 0
        });
        let status: RuntimeStatus = serde_json::from_value(original.clone()).unwrap();
        assert!(!status.agent_filesystem);
        assert!(!status.agent_model_catalog);
        assert!(!status.agent_skill_install);
        assert!(!status.agent_skill_upgrade);
        assert!(!status.agent_browser);
        assert!(!status.agent_resource_preparation);
        assert!(!status.agent_process_restart);
        assert!(!status.exact_github_credential_route);
        assert_eq!(serde_json::to_value(status).unwrap(), original);
    }

    #[test]
    fn browser_and_resource_preparation_capabilities_are_independent() {
        for browser in [false, true] {
            for preparation in [false, true] {
                let original = serde_json::json!({
                    "sandbox_id": "sandbox", "generation": 1,
                    "home_identity": "a".repeat(43), "boot_id": "a".repeat(32),
                    "installation_ready": true, "credential_ready": true,
                    "restore_pending": false, "agent_processes": 0,
                    "agent_browser": browser,
                    "agent_resource_preparation": preparation
                });
                let status: RuntimeStatus = serde_json::from_value(original).unwrap();
                status
                    .validate_for(&RuntimeStatusRequest {
                        sandbox_id: "sandbox".into(),
                        generation: 1,
                        home_identity: "a".repeat(43),
                    })
                    .unwrap();
                let restored: RuntimeStatus =
                    serde_json::from_value(serde_json::to_value(status).unwrap()).unwrap();
                assert_eq!(restored.agent_browser, browser);
                assert_eq!(restored.agent_resource_preparation, preparation);
            }
        }
    }
}
