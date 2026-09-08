use super::super::{agents_tests as agents, trigger_routes_tests::support_tests::TestApp};
use super::Scenario;
use layer2_core::sandbox::{AgentOrigin, AgentReadService, AgentStatus};
use layer2_core::trigger_domain::LaunchStoreError;
use layer2_core::{Authorization, Capability};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("source_") {
        return None;
    }
    assert!([
        "source_manual",
        "source_workspace",
        "source_missing_trigger",
        "source_forbidden",
        "source_anonymous",
        "source_foreign",
        "source_dependency",
        "source_instance_dependency"
    ]
    .contains(&name));
    let app = TestApp::new().await;
    let fixture = agents::fixture().await;
    let mut agent = agents::seeded_agent("agt_contract", AgentStatus::Ready, 2, None);
    agent.origin = match name {
        "source_workspace" => AgentOrigin::WorkspaceProfile { index: 0 },
        "source_missing_trigger" | "source_dependency" => AgentOrigin::Trigger {
            trigger_id: "trg_contract".into(),
            launch_id: "lch_contract".into(),
        },
        _ => AgentOrigin::Manual,
    };
    agents::insert_agent(&fixture.store, agent, vec![]).await;
    if name == "source_dependency" {
        app.launches.fail(
            "launch",
            LaunchStoreError::Unavailable("private dependency".into()),
        );
    }
    let reads = if name == "source_instance_dependency" {
        super::fault_cases::unavailable_instance_reads()
    } else {
        AgentReadService::new(fixture.store, Arc::new(agents::TestResources))
    };
    Some(Scenario {
        router: crate::rest::agent_source_router(reads, app.source_service),
        subject: match name {
            "source_anonymous" => None,
            "source_foreign" => Some("owner-b"),
            _ => Some("owner-a"),
        },
        authorization: if name == "source_forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::UseOwnSandboxes])
        },
        symbols: Default::default(),
    })
}
