//! Isolate the trace-capture contract from concurrently running routing tests.
//! Tracing callsite interest is process-wide even when dispatchers are scoped
//! to a thread; exercising those callsites first on threads without a subscriber
//! can otherwise suppress the captured events. The full hygiene assertions run
//! unchanged in their own test process with the production log filter.
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "audit_routing/hygiene.rs"]
mod hygiene;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod routing_fixture;
#[path = "audit_routing/support.rs"]
#[allow(dead_code)]
mod support;
