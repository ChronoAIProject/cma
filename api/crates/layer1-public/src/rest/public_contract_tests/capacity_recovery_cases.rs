//! Real HTTP handlers and resource authorization; durable outcomes are a port fixture.
use super::Scenario;
use crate::rest::{agent_capacity_recovery, shared_agent_tests};
use layer2_core::{
    sandbox::agent_capacity_recovery::preflight::CapacityRecoveryPreflightService, Authorization,
    Capability,
};
use std::sync::Arc;
mod commands;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("capacity_contract_")?;
    let (_, access, _, _) = shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let preflight = Arc::new(CapacityRecoveryPreflightService::new(access));
    let router = agent_capacity_recovery::agent_capacity_recovery_router(preflight.clone()).merge(
        agent_capacity_recovery::agent_capacity_recovery_commands_router(Arc::new(
            commands::Commands {
                preflight,
                mode: mode.to_owned(),
            },
        )),
    );
    Some(Scenario {
        router,
        subject: match mode {
            "anonymous" => None,
            "foreign" => Some("foreign"),
            _ => Some("owner-a"),
        },
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        symbols: Default::default(),
    })
}
