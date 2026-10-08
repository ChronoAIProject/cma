//! Golden compatibility suites: what the gateway sends today, pinned byte for
//! byte so that later changes provably keep legacy channels unchanged. Each
//! invariant has its own module; fixtures live under `golden/fixtures/`.
#[path = "golden/app.rs"]
mod app;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "golden/fixture.rs"]
mod fixture;
#[path = "golden/identities.rs"]
mod identities;
#[path = "latency/identity.rs"]
pub mod identity;
#[path = "golden/previous_schema.rs"]
mod previous_schema;
#[path = "golden/provider_capture.rs"]
mod provider_capture;
#[path = "golden/provider_normalize.rs"]
mod provider_normalize;
#[path = "golden/references.rs"]
mod references;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "golden/reseal.rs"]
mod reseal;
#[path = "golden/responses.rs"]
mod responses;
#[path = "relay/signer.rs"]
pub mod signer;
