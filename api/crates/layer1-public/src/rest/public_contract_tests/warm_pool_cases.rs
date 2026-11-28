//! Native pool wire contracts exercise the actual authenticated REST handlers.
use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::{Authorization, Caller, Capability};

use super::super::admin::{tests::fixture_with_pools, warm_pools::*};
use super::Scenario;

struct Pools(Option<WarmPoolError>);

fn view() -> WarmPoolView {
    WarmPoolView {
        id: "wp_contract".into(),
        revision: 1,
        config_sha256: "c".repeat(64),
        image: format!("registry/runtime@sha256:{}", "a".repeat(64)),
        cpu_millis: 1000,
        memory_mib: 1024,
        target_replicas: 2,
        provider_target_replicas: None,
        ready_replicas: None,
        observed: false,
        status: "pending".into(),
        retirement_status: None,
        resource_name: None,
        error: None,
        machine: false,
        current_runtime: None,
    }
}

/// The deployment-seeded shared desktop pool listed beside an Agent pool.
fn desktop_view() -> WarmPoolView {
    WarmPoolView {
        id: "wp_contract_desktop".into(),
        config_sha256: "e".repeat(64),
        image: format!("registry/desktop@sha256:{}", "b".repeat(64)),
        machine: true,
        ..view()
    }
}

impl Pools {
    fn result<T>(&self, value: T) -> Result<T, WarmPoolError> {
        self.0.map_or(Ok(value), Err)
    }
}

#[async_trait]
impl WarmPoolAdministration for Pools {
    async fn list(&self, _: &Caller) -> Result<WarmPoolList, WarmPoolError> {
        let retiring_desktop = WarmPoolView {
            id: "wp_contract_retiring_desktop".into(),
            config_sha256: "f".repeat(64),
            resource_name: Some("pool-contract-retiring-desktop".into()),
            cpu_millis: 2000,
            target_replicas: 0,
            provider_target_replicas: Some(2),
            retirement_status: Some("pending".into()),
            ..desktop_view()
        };
        self.result(WarmPoolList {
            items: vec![
                WarmPoolView {
                    current_runtime: Some(true),
                    ..view()
                },
                WarmPoolView {
                    current_runtime: Some(true),
                    ..desktop_view()
                },
                WarmPoolView {
                    current_runtime: Some(false),
                    ..retiring_desktop
                },
            ],
            default_image: view().image,
            default_cpu_millis: 1000,
            default_memory_mib: 1024,
            default_config_sha256: Some("d".repeat(64)),
            available: true,
            machine_image: Some(desktop_view().image),
            machine_config_sha256: Some("e".repeat(64)),
            default_claim_pool_id: None,
            machine_claim_pool_id: Some(desktop_view().id),
            default_capacity_pool_id: None,
            machine_capacity_pool_id: Some(desktop_view().id),
            runtime_versions: vec![WarmPoolRuntimeVersion {
                image: desktop_view().image,
                version: "1.2.3".into(),
            }],
        })
    }

    async fn configure(
        &self,
        _: &Caller,
        _: &str,
        body: ConfigureWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError> {
        self.result(WarmPoolView {
            cpu_millis: body.cpu_millis,
            memory_mib: body.memory_mib,
            target_replicas: body.target_replicas,
            ..view()
        })
    }

    async fn resize(
        &self,
        _: &Caller,
        _: &str,
        body: ResizeWarmPoolBody,
    ) -> Result<WarmPoolView, WarmPoolError> {
        self.result(WarmPoolView {
            revision: body.expected_revision + 1,
            target_replicas: body.target_replicas,
            ..view()
        })
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let fault = match name {
        "warm_pools_operator" | "warm_pools_anonymous" | "warm_pools_forbidden" => None,
        "warm_pools_unavailable" => Some(WarmPoolError::Unavailable),
        "warm_pools_conflict" => Some(WarmPoolError::Conflict),
        "warm_pools_invalid" => Some(WarmPoolError::Invalid),
        "warm_pools_missing" => Some(WarmPoolError::NotFound),
        _ => return None,
    };
    Some(Scenario {
        router: fixture_with_pools(false, Some(Arc::new(Pools(fault)))).app,
        subject: (name != "warm_pools_anonymous").then_some("operator"),
        authorization: Authorization::new(
            (name != "warm_pools_forbidden").then_some(Capability::ConfigurePlatform),
        ),
        symbols: Default::default(),
    })
}
