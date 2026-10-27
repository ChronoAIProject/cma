//! The single gate every management write of a channel definition passes.
//!
//! Readers of a new stored shape ship in an image before any writer of it
//! (phase R before phase W): an image that cannot parse a stored definition
//! refuses its channel, so storing a shape before every replica and every
//! rollback target reads it would break channels during a rollout. Until an
//! extension's flag is on, a write may therefore store only what an older
//! image reads, except for two writes that never introduce anything new:
//! carrying an existing extension through unchanged, and removing extensions.
//!
//! The gate runs before any other definition check and before the provider
//! sees the write, so a refused write reports exactly the error it got
//! before readers widened and never touches the provider.
use cmaeg_core::{
    channel::{
        ChannelDefinition, Extensions, PartitionRule, Platform, RoutingPolicy, Secret, Source,
    },
    ids::ConsumerId,
    ValidationError,
};

/// The refusal of a legacy-valid write that still introduces a source extension.
pub const DYNAMIC_SOURCES_DISABLED: &str = "dynamic_sources_disabled";
/// Refusal of new or changed raw-event verification sources.
pub const RELAY_RAW_VERIFICATION_DISABLED: &str = "relay_raw_verification_writes_disabled";
/// The refusal of a write that introduces or modifies routing.
pub const MULTI_CONSUMER_DISABLED: &str = "multi_consumer_disabled";

/// Flags that admit writes of new stored shapes. Default off until explicitly enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteFlags {
    /// Admits explicit native bot mention opt-ins after all readers support them.
    pub native_bot_mentions: bool,
    /// Admits verified native thread contracts after every reader supports them.
    pub thread_contract: bool,
    /// Admits routing to several consumers; wired by #998.
    pub multi_consumer: bool,
    /// Admits extended source and partition shapes; wired by #999.
    pub dynamic_sources: bool,
    /// Admits Discord/WhatsApp raw verification without enabling dynamic topology.
    pub relay_raw_verification: bool,
    /// Exact proxy bindings are admitted only after compatible readers deploy.
    pub lark_resources: bool,
    /// Admits native Discord sources and credential revisions after reader rollout.
    pub native_whatsapp: bool,
    pub native_discord: bool,
    pub telegram_message_events: bool,
    pub discord_thread_history: bool,
}

/// Decide whether a management write may store `next`, replacing `current`
/// (`None` for a create). The caller reports a refusal as an invalid request.
///
/// In order:
/// 1. a legacy-compatible `next` is allowed: today's path, or a write that
///    only removes extensions (any legacy error left in it is reported by the
///    reader, identically, because the reader applies the legacy rules to it);
/// 2. an extension `next` carries through unchanged from an extended
///    `current` is allowed, so an older client's full `PUT` keeps working;
/// 3. any other extension requires its flag. While it is off, a source
///    write the legacy rules refuse gets that legacy error unchanged, a
///    legacy-valid one that still carries `custom_key` gets
///    `dynamic_sources_disabled`, and a write that introduces or modifies
///    routing gets `multi_consumer_disabled`.
pub(crate) fn check(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
    flags: WriteFlags,
) -> Result<(), ValidationError> {
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::Telegram {
                message_events: true,
                ..
            }
        ) && !flags.telegram_message_events
            && !current.is_some_and(|old| old.sources.contains(source))
    }) {
        return Err(ValidationError::new(
            format!("/sources/{index}/message_events"),
            "native_telegram_message_events_writes_disabled",
        ));
    }
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::NyxidRelay {
                lark_resources: Some(_),
                ..
            }
        ) && !flags.lark_resources
            && !current.is_some_and(|old| old.sources.contains(source))
    }) {
        return Err(ValidationError::new(
            format!("/sources/{index}/lark_resources"),
            "lark_resource_writes_disabled",
        ));
    }
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::Discord {
                thread_history: true,
                ..
            }
        ) && !flags.discord_thread_history
            && !current.is_some_and(|old| old.sources.contains(source))
    }) {
        return Err(ValidationError::new(
            format!("/sources/{index}/thread_history"),
            "discord_thread_history_disabled",
        ));
    }
    if next.reply.thread_contract.is_some()
        && !flags.thread_contract
        && !current.is_some_and(|old| old.reply.thread_contract == next.reply.thread_contract)
    {
        return Err(ValidationError::new(
            "/reply/thread_contract",
            "thread_contract_writes_disabled",
        ));
    }
    let bot_sources = |d: &ChannelDefinition| {
        d.sources
            .iter()
            .filter(|s| s.native_bot_mentions())
            .cloned()
            .collect::<Vec<_>>()
    };
    let bots = bot_sources(next);
    if !bots.is_empty()
        && !flags.native_bot_mentions
        && !current.is_some_and(|old| bot_sources(old) == bots)
    {
        return Err(ValidationError::new(
            "/sources",
            "native_bot_mentions_writes_disabled",
        ));
    }
    if let Some(index) = next.sources.iter().position(|source| {
        matches!(
            source,
            Source::NyxidRelay {
                verify_raw_event: true,
                ..
            }
        )
    }) {
        if !flags.relay_raw_verification
            && !current.is_some_and(|old| SourceExtension::of(old) == SourceExtension::of(next))
        {
            return Err(ValidationError::new(
                format!("/sources/{index}/verify_raw_event"),
                RELAY_RAW_VERIFICATION_DISABLED,
            ));
        }
    }
    let whatsapp = |d: &ChannelDefinition| {
        (
            d.sources
                .iter()
                .filter(|s| matches!(s, Source::Whatsapp { .. }))
                .cloned()
                .collect::<Vec<_>>(),
            d.secrets.whatsapp.clone(),
        )
    };
    let next_whatsapp = whatsapp(next);
    if (!next_whatsapp.0.is_empty() || next_whatsapp.1.is_some())
        && !flags.native_whatsapp
        && !current.is_some_and(|old| whatsapp(old) == next_whatsapp)
    {
        return Err(ValidationError::new(
            "/sources",
            "native_whatsapp_writes_disabled",
        ));
    }
    let discord = |d: &ChannelDefinition| {
        (
            d.sources
                .iter()
                .filter(|s| matches!(s, Source::Discord { .. }))
                .cloned()
                .collect::<Vec<_>>(),
            d.secrets.discord_bot_token.clone(),
        )
    };
    let next_discord = discord(next);
    if (!next_discord.0.is_empty() || next_discord.1.is_some())
        && !flags.native_discord
        && !current.is_some_and(|old| discord(old) == next_discord)
    {
        return Err(ValidationError::new(
            "/sources",
            "native_discord_writes_disabled",
        ));
    }
    let extensions = Extensions::of(next);
    if extensions.legacy_compatible() {
        return Ok(());
    }
    let carried_sources =
        current.is_some_and(|current| carries_through(current, next, Extension::Sources));
    let carried_routing =
        current.is_some_and(|current| carries_through(current, next, Extension::Routing));
    let sources = !dynamic_sources(next) || flags.dynamic_sources || carried_sources;
    let routing = !extensions.routing || flags.multi_consumer || carried_routing;
    if sources && routing {
        if carried_sources || carried_routing {
            tracing::debug!(
                sources = carried_sources,
                routing = carried_routing,
                "channel write carries its extensions through unchanged"
            );
        }
        return Ok(());
    }
    let error = if !sources {
        match next.validate_legacy() {
            Err(error) => error,
            // This optional partition shape is independent from raw verification.
            Ok(()) => ValidationError::new("/partition/custom_key", DYNAMIC_SOURCES_DISABLED),
        }
    } else if next.routing.is_some() {
        ValidationError::new("/routing", MULTI_CONSUMER_DISABLED)
    } else {
        ValidationError::new("/secrets/consumer_keys", MULTI_CONSUMER_DISABLED)
    };
    tracing::info!(
        pointer = %error.pointer,
        reason = %error.message,
        sources = extensions.sources,
        routing = extensions.routing,
        "channel write refused by the write gate"
    );
    Err(error)
}

fn dynamic_sources(definition: &ChannelDefinition) -> bool {
    let mut legacy = definition.clone();
    legacy
        .sources
        .retain(|s| !matches!(s, Source::Discord { .. } | Source::Whatsapp { .. }));
    if legacy.sources.is_empty() && !definition.sources.is_empty() {
        return definition.partition.custom_key.is_some();
    }
    legacy.validate_legacy_sources_and_partition().is_err()
        || definition.partition.custom_key.is_some()
}
fn without_relay_verification(sources: &[Source]) -> Vec<Source> {
    sources
        .iter()
        .cloned()
        .map(|mut source| {
            if let Source::NyxidRelay {
                platform: Platform::Discord | Platform::Whatsapp,
                verify_raw_event,
                trust_normalized,
                ..
            } = &mut source
            {
                *verify_raw_event = false;
                *trust_normalized = false;
            }
            source
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Extension {
    Sources,
    Routing,
}

/// Whether `current` uses `extension` and `next` keeps it intact.
fn carries_through(
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

#[cfg(test)]
#[path = "write_gate_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "raw_verification_tests.rs"]
mod raw_verification_tests;
