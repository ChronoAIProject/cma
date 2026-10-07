#[path = "fake_provider.rs"]
pub mod fake_provider;
/// The media test helpers, shared with the media tests.
pub mod media {
    #[path = "fakes.rs"]
    pub mod fakes;
    #[path = "image_fixtures.rs"]
    pub mod image_fixtures;
    #[path = "support.rs"]
    pub mod support;
}
#[path = "tools/attachments.rs"]
mod attachments;
#[path = "tools/attachments_mcp.rs"]
mod attachments_mcp;
#[path = "tools/media_support.rs"]
pub mod media_support;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "tools/rest.rs"]
mod rest;
#[path = "tools/rpc.rs"]
mod rpc;
#[path = "relay/signer.rs"]
pub mod signer;
#[path = "tools/support.rs"]
mod support;
