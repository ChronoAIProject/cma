use super::Scenario;
use crate::rest::instances_tests::prepare_recovery_tests::recovery_fixture;
use layer2_core::{Authorization, Capability};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("workspace_prepare_recovery_")?;
    assert!(["owner", "admin", "anonymous", "forbidden", "foreign"].contains(&mode));
    let fixture = recovery_fixture().await;
    Some(Scenario {
        router: fixture.router,
        subject: match mode {
            "anonymous" => None,
            "admin" => Some("operator"),
            "foreign" => Some("foreign"),
            _ => Some("owner"),
        },
        authorization: match mode {
            "admin" => Authorization::new([Capability::ControlAnySandbox]),
            "forbidden" => Authorization::none(),
            _ => Authorization::new([Capability::UseOwnSandboxes]),
        },
        symbols: Default::default(),
    })
}
