use std::sync::Arc;

use crate::rest::{agents::agent_router, agents_tests};
use layer2_core::{
    sandbox::{store::AgentRestartAcceptance, AgentRestartControl, AgentServiceError},
    AgentId, Authorization, Caller, Capability,
};
use serde_json::json;

struct Restart {
    refusal: Option<AgentServiceError>,
}

#[async_trait::async_trait]
impl AgentRestartControl for Restart {
    async fn restart(
        &self,
        caller: &Caller,
        id: &AgentId,
        key: &str,
    ) -> Result<AgentRestartAcceptance, AgentServiceError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(AgentServiceError::Forbidden);
        }
        if caller.subject.as_str() != "owner-a" || id.as_str() != "agt_restart_contract" {
            return Err(AgentServiceError::NotFound);
        }
        assert_eq!(key, "owner-restart");
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let at = "2026-09-05T00:00:00Z";
        Ok(serde_json::from_value(json!({
            "operation_id": "restart-fixture",
            "agent_id": id,
            "worktree_id": "wkt_restart_contract",
            "revision": 8,
            "accepted_at": at,
            "next_incarnation": 2,
            "predecessor_stop_operation": "stop-fixture",
            "source": {
                "conversation_id": "retained-conversation",
                "predecessor_open": {
                    "step": "open_conversation", "operation_id": "open-fixture",
                    "request_sha256": "a".repeat(64), "completed_at": at
                },
                "predecessor_close": {
                    "step": "close_process", "operation_id": "close-fixture",
                    "request_sha256": "b".repeat(64), "completed_at": at
                },
                "runtime": {
                    "runtime": {"sandbox_id": "sbx_route", "generation": 1, "home_identity": "c".repeat(43)},
                    "boot_id": "d".repeat(32)
                },
                "worktree_revision": 1, "workspace_revision": 1, "sandbox_revision": 1
            }
        })).unwrap())
    }
}

pub(super) async fn scenario(name: &str) -> Option<super::Scenario> {
    let mode = name.strip_prefix("owner_agent_restart_")?;
    let fixture = agents_tests::fixture().await;
    let state = if mode == "disabled" {
        fixture.state
    } else {
        fixture.state.with_owner_agent_restart(Arc::new(Restart {
            refusal: match mode {
                "runtime_unsupported" => Some(AgentServiceError::RestartRuntimeUnsupported),
                "turn_unsettled" => Some(AgentServiceError::RestartTurnUnsettled),
                _ => None,
            },
        }))
    };
    Some(super::Scenario {
        router: agent_router(state),
        subject: match mode {
            "anonymous" => None,
            "foreign" => Some("owner-b"),
            _ => Some("owner-a"),
        },
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::UseOwnSandboxes])
        },
        symbols: Default::default(),
    })
}
