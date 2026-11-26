//! Lifecycle wire projections over real REST handlers; durable store tests live in CMA.
use super::Scenario;
use crate::rest::*;
use layer2_core::{resource_access::CallerContext, Authorization, Caller, Capability};
use std::sync::Arc;

struct Lifecycle(String);
#[async_trait::async_trait]
impl AgentLifecycleProvisioner for Lifecycle {
    async fn desktop_capability(
        &self,
        _: &Caller,
        _: Option<&CallerContext>,
        scope: DesktopCreationScope,
    ) -> Result<
        layer2_core::sandbox::desktop_creation::DesktopCapability,
        AgentLifecycleProvisioningError,
    > {
        use layer2_core::sandbox::desktop_creation::{DesktopAvailability, DesktopCapability};
        if scope.workspace_id.is_some() && scope.sandbox_id.is_some() {
            return Err(AgentLifecycleProvisioningError::Invalid(
                "workspace_id and sandbox_id are mutually exclusive".into(),
            ));
        }
        if self.0 == "unavailable" {
            return Err(AgentLifecycleProvisioningError::Unavailable);
        }
        if self.0 == "foreign" {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        let mut capability =
            DesktopCapability::unavailable("This deployment has no verified desktop runtime.");
        if self.0 == "desktop_available" {
            capability.state = DesktopAvailability::Available;
            capability.reason = "Verified desktop runtime.".into();
            capability.runtime_image =
                Some(format!("proof.invalid/runtime@sha256:{}", "d".repeat(64)));
        }
        if self.0 == "desktop_pending" {
            capability.state = DesktopAvailability::TemporarilyUnavailable;
            capability.reason =
                "Runtime capability could not be verified. Retry the same request.".into();
        }
        Ok(capability)
    }
    async fn create(
        &self,
        _: &Caller,
        _: Option<&CallerContext>,
        _: Option<&str>,
        _: AgentLifecycleCreateRequest,
        _: &str,
    ) -> Result<AgentLifecycleCreateReceipt, AgentLifecycleProvisioningError> {
        match self.0.as_str() {
            "quota" => Err(AgentLifecycleProvisioningError::SandboxLimitReached {
                limit: 10,
                live: 10,
                quota_usage: None,
            }),
            "desktop_unsupported" => Err(AgentLifecycleProvisioningError::DesktopUnsupported(
                "This deployment has no verified desktop runtime.".into(),
            )),
            "unavailable" => Err(AgentLifecycleProvisioningError::Unavailable),
            "image_auth" => Err(AgentLifecycleProvisioningError::SignInRequired),
            "image_forbidden" => Err(AgentLifecycleProvisioningError::ImageAccessDenied),
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
    async fn cancel_unallocated(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<AgentLifecycleOperationView, AgentLifecycleOperationReadError> {
        if self.0 == "forbidden" {
            return Err(AgentLifecycleOperationReadError::Forbidden);
        }
        if self.0 == "allocated" {
            return Err(AgentLifecycleOperationReadError::Conflict(
                "Creation is already allocated".into(),
            ));
        }
        let mut view = self.read(caller, id).await?;
        view.status = "failed".into();
        view.phase = "failed".into();
        view.error = Some("Creation cancelled before Sandbox allocation".into());
        view.connection = None;
        Ok(view)
    }
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
            accepted_at: None,
            failure: None,
            callback_url: None,
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
                repository_access: None,
                repository_scope_repair: None,
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

#[tokio::test]
async fn acceptance_time_is_optional_and_serializes_the_supplied_admission_time() {
    let caller = Caller::new(
        layer2_core::SubjectId::new("owner-a"),
        Authorization::none(),
    );
    let mut view = AgentLifecycleOperationReader::read(
        &Lifecycle("ready".into()),
        &caller,
        "operation-contract",
    )
    .await
    .unwrap();
    assert!(serde_json::to_value(&view)
        .unwrap()
        .get("accepted_at")
        .is_none());
    view.accepted_at = Some("2026-10-12T23:59:00Z".into());
    assert_eq!(
        serde_json::to_value(&view).unwrap()["accepted_at"],
        "2026-10-12T23:59:00Z"
    );
}
