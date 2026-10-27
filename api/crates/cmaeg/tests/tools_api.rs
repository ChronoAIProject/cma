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
#[path = "tools/attachment_capabilities.rs"]
mod attachment_capabilities;
#[path = "tools/attachment_transformer.rs"]
mod attachment_transformer;
#[path = "tools/attachments.rs"]
mod attachments;
#[path = "tools/attachments_mcp.rs"]
mod attachments_mcp;
#[path = "tools/complete_budget.rs"]
mod complete_budget;
#[path = "tools/consumer_auth.rs"]
mod consumer_auth;
#[path = "tools/consumer_context.rs"]
mod consumer_context;
#[path = "tools/consumer_media.rs"]
mod consumer_media;
#[path = "tools/consumer_support.rs"]
pub mod consumer_support;
#[path = "tools/download_support.rs"]
pub mod download_support;
#[path = "tools/link_redemption.rs"]
mod link_redemption;
#[path = "tools/media_downloads.rs"]
mod media_downloads;
#[path = "tools/media_links.rs"]
mod media_links;
#[path = "tools/media_support.rs"]
pub mod media_support;
#[path = "tools/native_discord_edits.rs"]
mod native_discord_edits;
#[path = "relay/support.rs"]
pub mod relay_support;
#[path = "tools/rest.rs"]
mod rest;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
pub mod routing_fixture;
#[path = "tools/rpc.rs"]
mod rpc;
#[path = "relay/signer.rs"]
pub mod signer;
#[path = "tools/support.rs"]
mod support;
#[path = "tools/whatsapp_templates.rs"]
mod whatsapp_templates;

#[path = "tools/reply_idempotency.rs"]
mod reply_idempotency;
