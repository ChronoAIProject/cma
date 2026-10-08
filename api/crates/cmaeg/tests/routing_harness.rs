//! Real gateway components with fake services only at external HTTP boundaries.
#[path = "routing_harness/app.rs"]
mod app;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "golden/fixture.rs"]
mod fixture;
#[path = "golden/kill_switch.rs"]
mod kill_switch;
#[path = "routing_harness/latency.rs"]
mod latency;
#[path = "routing_harness/logs.rs"]
mod logs;
#[path = "routing_harness/manual.rs"]
mod manual;
#[path = "routing_harness/peers.rs"]
mod peers;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod policy;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "routing_harness/requests.rs"]
mod requests;
#[path = "routing_harness/sdk.rs"]
mod sdk;
#[path = "relay/signer.rs"]
pub mod signer;
#[path = "routing_harness/smoke.rs"]
mod smoke;
