use super::Scenario;
use layer2_core::{Authorization, Capability};
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("runtime_update_")?;
    let f =
        super::super::instances_tests::runtime_update_tests::update_fixture(mode != "unconfigured")
            .await;
    Some(Scenario {
        router: f.router,
        subject: if mode == "anonymous" {
            None
        } else if mode == "foreign" {
            Some("foreign")
        } else {
            Some("owner")
        },
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::UseOwnSandboxes])
        },
        symbols: Default::default(),
    })
}
