//! The multi-consumer rollback audit (invariant I12), against the fake CMA
//! record store and the fake provider.
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "audit_routing/report.rs"]
mod report;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod routing_fixture;
#[path = "audit_routing/support.rs"]
mod support;
