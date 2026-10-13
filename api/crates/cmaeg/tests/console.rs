//! The gateway console: enablement, sign-in and per-request authority, each
//! against the fake NyxID's OAuth endpoints.
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;

#[path = "console/authority.rs"]
mod authority;
#[path = "console/enablement.rs"]
mod enablement;
#[path = "console/hygiene.rs"]
mod hygiene;
#[path = "console/memory.rs"]
mod memory;
#[path = "console/memory_routing.rs"]
mod memory_routing;
#[path = "console/observations.rs"]
mod observations;
#[path = "console/owner.rs"]
mod owner;
#[path = "console/pages.rs"]
mod pages;
#[path = "console/pages_authority.rs"]
mod pages_authority;
#[path = "console/pages_changes.rs"]
pub mod pages_changes;
#[path = "console/pages_event.rs"]
mod pages_event;
#[path = "console/pages_hygiene.rs"]
mod pages_hygiene;
#[path = "console/pages_link.rs"]
mod pages_link;
#[path = "console/pages_routing.rs"]
mod pages_routing;
#[path = "console/pages_sources.rs"]
mod pages_sources;
#[path = "console/pages_support.rs"]
pub mod pages_support;
#[path = "console/pages_tabs.rs"]
mod pages_tabs;
#[path = "console/pages_trace.rs"]
mod pages_trace;
#[path = "console/protection.rs"]
mod protection;
#[path = "console/routed_support.rs"]
pub mod routed_support;
#[path = "console/sessions.rs"]
mod sessions;
#[path = "console/signin.rs"]
mod signin;
#[path = "console/summary_bounds.rs"]
mod summary_bounds;
#[path = "console/support.rs"]
pub mod support;

#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
pub mod policy;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "../../cmaeg-core/tests/routing_resolver/fixture.rs"]
pub mod resolver_fixture;
#[path = "relay/signer.rs"]
pub mod signer;
