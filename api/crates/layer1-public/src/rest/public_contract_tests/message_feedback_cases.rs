use layer2_core::{Authorization, Capability};

use super::Scenario;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("message_feedback_")?;
    let (mut fixture, _, _) =
        super::super::agents_tests::message_feedback::feedback_fixture().await;
    if mode == "unavailable" {
        fixture.state.message_feedback = None;
        fixture.router = super::super::agent_router(fixture.state);
    }
    Some(Scenario {
        router: fixture.router,
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
