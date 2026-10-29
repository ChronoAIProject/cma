use super::Scenario;
use crate::rest::agents_tests::{insert_agent, seeded_agent, worktree_recovery::recovery_fixture};
use layer2_core::sandbox::AgentStatus;
use layer2_core::{Authorization, Capability};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("agent_worktree_recovery_")?;
    assert!([
        "owner",
        "admin",
        "anonymous",
        "forbidden",
        "foreign",
        "limit"
    ]
    .contains(&mode));
    let fixture = recovery_fixture().await;
    if mode == "limit" {
        for index in 0..4 {
            insert_agent(
                &fixture.store,
                seeded_agent(&format!("agt_live_{index}"), AgentStatus::Ready, 0, None),
                vec![],
            )
            .await;
        }
    }
    Some(Scenario {
        router: fixture.router,
        subject: match mode {
            "anonymous" => None,
            "admin" => Some("operator"),
            "foreign" => Some("foreign"),
            _ => Some("owner-a"),
        },
        authorization: match mode {
            "admin" => Authorization::new([Capability::ControlAnySandbox]),
            "forbidden" => Authorization::none(),
            _ => Authorization::new([Capability::UseOwnSandboxes]),
        },
        symbols: Default::default(),
    })
}
