//! The multi-consumer rollback audit (invariant I12), against the fake CMA
//! record store and the fake provider.
#[path = "audit_routing/clear.rs"]
mod clear;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "audit_routing/hygiene.rs"]
mod hygiene;
#[path = "golden/previous_schema.rs"]
mod previous_schema;
#[path = "audit_routing/report.rs"]
mod report;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod routing_fixture;
#[path = "audit_routing/support.rs"]
mod support;
