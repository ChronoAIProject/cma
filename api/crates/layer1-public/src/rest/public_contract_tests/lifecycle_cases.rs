//! Lifecycle wire projections over real REST handlers; durable store tests live in CMA.
use super::Scenario;
use crate::rest::*;
use layer2_core::{resource_access::CallerContext, Authorization, Caller, Capability};
use std::sync::Arc;

struct Lifecycle(String);
#[async_trait::async_trait]
impl AgentLifecycleProvisioner for Lifecycle {
    async fn create(
        &self,
        _: &Caller,
        _: Option<&CallerContext>,
        _: Option<&str>,
        _: AgentLifecycleCreateRequest,
        _: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        match self.0.as_str() {
            "unavailable" => Err(AgentLifecycleProvisioningError::Unavailable),
            "connection" => Err(AgentLifecycleProvisioningError::ConnectionRequired {
                services: vec!["api-github".into()],
                connection_target: serde_json::json!({"type":"workspace","id":"workspace-contract"}),
            }),
            _ => Ok(AgentLifecycleCreateReceipt {
                id: "operation-contract".into(),
                status: "provisioning".into(),
            }),
        }
    }
}
#[async_trait::async_trait]
impl AgentLifecycleOperationReader for Lifecycle {
    async fn read(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        match self.0.as_str() {
            "unavailable" => return Err(AgentLifecycleOperationReadError::Unavailable),
            "foreign" => return Err(AgentLifecycleOperationReadError::NotFound),
            _ => {}
        }
        Ok(AgentLifecycleOperationView {
            id: "operation-contract".into(),
            status: "waiting_for_connection".into(),
            phase: "sandbox_ready".into(),
            agent_id: None,
            error: None,
            events: vec![AgentLifecycleEventView {
                sequence: 2,
                phase: "sandbox_ready".into(),
                status: "ready".into(),
                message: "Sandbox ready".into(),
                occurred_at: "2026-10-13T00:00:00Z".into(),
            }],
            connection: Some(AgentLifecycleConnectionRequired {
                services: vec!["api-github".into()],
                target: serde_json::json!({"type":"sandbox_profile","profile":{"id":"internal-sandbox","revision":1}}),
            }),
        })
    }
}
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("lifecycle_contract_")?;
    let fixture = super::super::agents_tests::fixture().await;
    let port = Arc::new(Lifecycle(mode.into()));
    Some(Scenario {
        router: agent_router(
            fixture
                .state
                .with_lifecycle(port.clone())
                .with_lifecycle_operations(port),
        ),
        subject: (mode != "anonymous").then_some("owner-a"),
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox])
        },
        symbols: Default::default(),
    })
}
