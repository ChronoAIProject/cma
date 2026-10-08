//! The gateway console: enablement, sign-in and per-request authority, each
//! against the fake NyxID's OAuth endpoints.
#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;

#[path = "console/authority.rs"]
mod authority;
#[path = "console/enablement.rs"]
mod enablement;
#[path = "console/hygiene.rs"]
mod hygiene;
#[path = "console/observations.rs"]
mod observations;
#[path = "console/owner.rs"]
mod owner;
#[path = "console/protection.rs"]
mod protection;
#[path = "console/sessions.rs"]
mod sessions;
#[path = "console/signin.rs"]
mod signin;
#[path = "console/support.rs"]
pub mod support;

#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "relay/signer.rs"]
pub mod signer;
