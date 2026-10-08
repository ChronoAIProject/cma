//! Routing through real authenticated ingress and the production fan-out decorator.
#[path = "routing/chat.rs"]
mod chat;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "routing/notices.rs"]
mod notices;
#[path = "routing/projection.rs"]
mod projection;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
pub mod routing_fixture;
#[path = "relay/signer.rs"]
pub mod signer;
#[path = "routing/support.rs"]
mod support;

#[path = "routing/custom.rs"]
mod custom;
#[path = "routing/custom_stream.rs"]
mod custom_stream;

#[path = "routing/cards.rs"]
mod cards;
#[path = "fake_provider.rs"]
pub mod fake_provider;

#[path = "routing/privacy.rs"]
mod privacy;

#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;
#[path = "routing/management_support.rs"]
mod management_support;
#[path = "routing/management_walkthrough.rs"]
mod management_walkthrough;
