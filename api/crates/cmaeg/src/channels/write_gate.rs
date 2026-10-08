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
    channel::{ChannelDefinition, Extensions, PartitionRule, Secret, Source},
    ValidationError,
};

/// The refusal of a legacy-valid write that still introduces a source extension.
pub const DYNAMIC_SOURCES_DISABLED: &str = "dynamic_sources_disabled";
/// The refusal of a legacy-valid write that introduces routing.
pub const MULTI_CONSUMER_DISABLED: &str = "multi_consumer_disabled";

/// Flags that admit writes of new stored shapes. All false until the flags exist.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteFlags {
    /// Admits routing to several consumers; wired by #998.
    pub multi_consumer: bool,
    /// Admits extended source and partition shapes; wired by #999.
    pub dynamic_sources: bool,
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
/// 3. any other extension requires its flag. While it is off, a write the
///    legacy rules refuse gets that legacy error unchanged, and a
///    legacy-valid write that still carries an extension gets
///    `dynamic_sources_disabled` (or `multi_consumer_disabled`).
pub(crate) fn check(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
    flags: WriteFlags,
) -> Result<(), ValidationError> {
    let extensions = Extensions::of(next);
    if extensions.legacy_compatible() {
        return Ok(());
    }
    let carried = current.is_some_and(|current| carries_sources_through(current, next));
    let sources = !extensions.sources || flags.dynamic_sources || carried;
    let routing = !extensions.routing || flags.multi_consumer;
    if sources && routing {
        if carried {
            tracing::debug!("channel write carries its source extension through unchanged");
        }
        return Ok(());
    }
    let error = match next.validate_legacy() {
        Err(error) => error,
        Ok(()) if !sources => ValidationError::new("/sources", DYNAMIC_SOURCES_DISABLED),
        Ok(()) => ValidationError::new("/", MULTI_CONSUMER_DISABLED),
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

/// Whether `current` is extended and `next` keeps its source extension intact.
fn carries_sources_through(current: &ChannelDefinition, next: &ChannelDefinition) -> bool {
    Extensions::of(current).sources && SourceExtension::of(current) == SourceExtension::of(next)
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
            telegram_webhook_secret: secrets.telegram_webhook_secret.as_ref(),
            hmac_secrets: &secrets.hmac_secrets,
        }
    }
}

#[cfg(test)]
#[path = "write_gate_tests.rs"]
mod tests;
