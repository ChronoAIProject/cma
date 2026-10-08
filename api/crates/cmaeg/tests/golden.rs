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
#[path = "latency/identity.rs"]
pub mod identity;
#[path = "golden/provider_capture.rs"]
mod provider_capture;
#[path = "golden/provider_normalize.rs"]
mod provider_normalize;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "relay/signer.rs"]
pub mod signer;
