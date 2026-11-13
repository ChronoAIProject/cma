use super::Scenario;
use layer2_core::{Authorization, Capability};
use std::sync::Arc;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if let Some(mode) = name.strip_prefix("retained_reopen_") {
        let f =
            super::super::instances_tests::runtime_update_tests::retained::reopen::reopen_fixture(
                mode,
            )
            .await;
        return Some(Scenario {
            router: f.router,
            subject: match mode {
                "anonymous" => None,
                "foreign" => Some("foreign"),
                _ => Some("owner"),
            },
            authorization: if mode == "forbidden" {
                Authorization::none()
            } else {
                Authorization::new([Capability::UseOwnSandboxes])
            },
            symbols: Default::default(),
        });
    }
    let mode = name.strip_prefix("retained_recovery_")?;
    use super::super::instances_tests::runtime_update_tests::{
        retained::NeverInspect, update_fixture,
    };
    let mut f = update_fixture(false).await;
    if mode != "unconfigured" {
        f.state = f.state.with_retained_recovery(Arc::new(
            layer2_core::sandbox::RetainedRecoveryService::new(
                f.store.clone(),
                Arc::new(NeverInspect),
            ),
        ));
        f.router = super::super::hierarchy_router(f.state);
    }
    Some(Scenario {
        router: f.router,
        subject: match mode {
            "anonymous" => None,
            "foreign" => Some("foreign"),
            _ => Some("owner"),
        },
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::UseOwnSandboxes])
        },
        symbols: Default::default(),
    })
}
