use super::Scenario;
use layer2_core::{Authorization, Capability};
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("runtime_update_")?;
    let f =
        super::super::instances_tests::runtime_update_tests::update_fixture(mode != "unconfigured")
            .await;
    if mode == "unavailable" {
        use layer2_core::sandbox::store::SandboxStore;
        let mut sandbox = f
            .store
            .get_sandbox(&layer2_core::SandboxId("sbx_update".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = sandbox.revision;
        sandbox.runtime.as_mut().unwrap().runtime_version = "runtime-v1".into();
        sandbox.revision += 1;
        f.store.save_sandbox(revision, sandbox).await.unwrap();
    }
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
