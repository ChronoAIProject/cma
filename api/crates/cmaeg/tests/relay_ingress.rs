#[path = "relay/keys.rs"]
mod keys;
#[path = "relay/lark_media.rs"]
mod lark_media;
#[path = "relay/lark_post.rs"]
mod lark_post;
#[path = "relay/lark_resources.rs"]
mod lark_resources;
#[path = "relay/multi_relay.rs"]
mod multi_relay;
#[path = "relay/normalization.rs"]
mod normalization;
#[path = "relay/nyxid_shapes.rs"]
mod nyxid_shapes;
#[path = "relay/platforms.rs"]
mod platforms;
#[path = "relay/routing_notice.rs"]
mod routing_notice;
#[path = "relay/signer.rs"]
mod signer;
#[path = "relay/support.rs"]
mod support;
#[path = "relay/telegram_media.rs"]
mod telegram_media;
#[path = "relay/transport.rs"]
mod transport;
#[path = "relay/verification.rs"]
mod verification;
#[path = "relay/verified_platforms.rs"]
mod verified_platforms;

#[path = "fake_nyxid_reply.rs"]
pub mod fake_nyxid_reply;
#[path = "fake_provider.rs"]
mod fake_provider;
#[path = "relay/runner_binary.rs"]
mod runner_binary;
#[path = "relay/runner_process.rs"]
mod runner_process;

#[path = "relay/managed_creation.rs"]
mod managed_creation;

#[path = "relay/thread_follow.rs"]
mod thread_follow;
use cmaeg::providers;
