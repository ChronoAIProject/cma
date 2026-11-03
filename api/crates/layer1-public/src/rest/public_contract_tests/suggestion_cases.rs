use super::Scenario;
use layer2_core::{Authorization, Capability};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("suggestions_")?;
    let (mut fixture, _, _) =
        super::super::agents_tests::message_feedback::feedback_fixture().await;
    if mode != "unavailable" {
        fixture.state = fixture.state.with_suggestions(fixture.store.clone());
    }
    Some(Scenario {
        router: super::super::agent_router(fixture.state),
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
