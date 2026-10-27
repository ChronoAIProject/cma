//! Retained source and routing extension comparisons for channel writes.
use super::{dynamic_sources, without_relay_verification};
use cmaeg_core::{
    channel::{ChannelDefinition, Extensions, PartitionRule, RoutingPolicy, Secret, Source},
    ids::ConsumerId,
};

#[derive(Clone, Copy)]
pub(super) enum Extension {
    Sources,
    Routing,
}

/// Whether `current` uses `extension` and `next` keeps it intact.
pub(super) fn carries_through(
    current: &ChannelDefinition,
    next: &ChannelDefinition,
    extension: Extension,
) -> bool {
    let extensions = Extensions::of(current);
    match extension {
        Extension::Sources => {
            if !dynamic_sources(current) {
                return false;
            }
            // A raw-only opt-in can preserve an existing broader source topology;
            // its separate gate above still checks exact records when disabled.
            let old_sources = without_relay_verification(&current.sources);
            let new_sources = without_relay_verification(&next.sources);
            SourceExtension {
                sources: &old_sources,
                ..SourceExtension::of(current)
            } == SourceExtension {
                sources: &new_sources,
                ..SourceExtension::of(next)
            }
        }
        Extension::Routing => {
            extensions.routing && RoutingExtension::of(current) == RoutingExtension::of(next)
        }
    }
}

/// Everything a routing extension consists of: the policy and the
/// additional consumers' keys.
#[derive(PartialEq)]
struct RoutingExtension<'a> {
    routing: Option<&'a RoutingPolicy>,
    consumer_keys: &'a [(ConsumerId, Secret)],
}

impl<'a> RoutingExtension<'a> {
    fn of(definition: &'a ChannelDefinition) -> Self {
        Self {
            routing: definition.routing.as_ref(),
            consumer_keys: &definition.secrets.consumer_keys,
        }
    }
}

/// Everything a source extension consists of: the sources, the partition
/// rule and the credentials those sources authenticate with. A write that
/// changes none of it introduces nothing an older image could not read
/// before.
#[derive(PartialEq)]
struct SourceExtension<'a> {
    sources: &'a [Source],
    partition: &'a PartitionRule,
    channel_key: Option<&'a Secret>,
    telegram_bot_token: Option<&'a Secret>,
    discord_bot_token: Option<&'a Secret>,
    telegram_webhook_secret: Option<&'a Secret>,
    hmac_secrets: &'a [(String, Secret)],
}

impl<'a> SourceExtension<'a> {
    fn of(definition: &'a ChannelDefinition) -> Self {
        let secrets = &definition.secrets;
        Self {
            sources: &definition.sources,
            partition: &definition.partition,
            channel_key: secrets.channel_key.as_ref(),
            telegram_bot_token: secrets.telegram_bot_token.as_ref(),
            discord_bot_token: secrets.discord_bot_token.as_ref(),
            telegram_webhook_secret: secrets.telegram_webhook_secret.as_ref(),
            hmac_secrets: &secrets.hmac_secrets,
        }
    }
}
