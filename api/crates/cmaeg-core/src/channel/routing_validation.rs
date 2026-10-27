//! The reader's rules for a routing policy. Every refusal names the exact
//! field, and the first refusal wins, so the order below is part of the
//! refusal contract.
use std::collections::BTreeSet;

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::{
    routing::{MAX_ADDITIONAL_CONSUMERS, MAX_MATCH_ENTRIES, MAX_MATCH_ENTRY_BYTES, MAX_RULES},
    validation::validate_hash,
    AgentEventPolicy, ChannelDefinition, Consumer, ConsumerReply, DefaultReply, ProviderBinding,
    ProviderTarget, PublishPolicy, RoutingPolicy, RuleMatch, Sink, Source, MAX_TARGETS,
    ROUTING_POLICY_VERSION,
};
use crate::{
    ids::{ConsumerId, ConsumerName},
    require_string, ValidationError,
};

const UNIQUE_ID: &str = "must be a unique consumer id";
const UNIQUE_NAME: &str = "must be a unique, unreserved consumer name";
const LABEL: &str = "must be 1 to 64 printable characters";
const KEY_REUSED: &str = "consumer_key_reused";
const DUPLICATE_AGENT_TARGET: &str = "duplicate_agent_target";
const ONE_KEY_PER_CONSUMER: &str = "must hold one key per consumer";
const MAX_LABEL_CHARS: usize = 64;
const MAX_CONSUMER_KEY_BYTES: usize = 4_096;

impl ChannelDefinition {
    /// Consumer keys belong to a routing policy's consumers, so they are
    /// refused without one; a policy is checked against the whole definition.
    pub(super) fn validate_routing(&self) -> Result<(), ValidationError> {
        match &self.routing {
            Some(routing) => routing.validate(self),
            None if !self.secrets.consumer_keys.is_empty() => Err(ValidationError::new(
                "/secrets/consumer_keys",
                ONE_KEY_PER_CONSUMER,
            )),
            None => Ok(()),
        }
    }
}

/// What every consumer must not share with another: its id, its name, its
/// agent key (CMA resolves one binding per key hash) and the existing agent
/// it borrows.
struct Seen<'a> {
    ids: BTreeSet<&'a ConsumerId>,
    names: BTreeSet<&'a str>,
    key_hashes: BTreeSet<&'a str>,
    agents: BTreeSet<&'a str>,
}

impl RoutingPolicy {
    /// The reader's rules, against the definition that holds the policy.
    pub(crate) fn validate(&self, def: &ChannelDefinition) -> Result<(), ValidationError> {
        if self.version != ROUTING_POLICY_VERSION {
            return Err(ValidationError::new(
                "/routing/version",
                "unsupported version",
            ));
        }
        if self.consumers.len() > MAX_ADDITIONAL_CONSUMERS {
            return Err(ValidationError::new(
                "/routing/consumers",
                "at most seven additional consumers",
            ));
        }
        let bots: Vec<String> = def
            .sources
            .iter()
            .filter_map(|source| match source {
                Source::Telegram { bot_username, .. } | Source::NyxidRelay { bot_username, .. } => {
                    bot_username.as_deref().map(str::to_ascii_lowercase)
                }
                Source::Custom { .. } | Source::Discord { .. } | Source::Whatsapp { .. } => None,
            })
            .collect();
        let known: BTreeSet<&ConsumerId> = self.consumers.iter().map(|c| &c.id).collect();
        let default = &self.default_consumer;
        let pointer = "/routing/default_consumer";
        validate_name(&default.name, &bots, &format!("{pointer}/name"))?;
        validate_label(default.label.as_deref(), &format!("{pointer}/label"))?;
        validate_policy(
            def,
            pointer,
            None,
            &def.provider,
            &default.reply,
            &default.publish,
            &known,
        )?;
        let mut seen = Seen {
            ids: BTreeSet::new(),
            names: BTreeSet::from([default.name.as_str()]),
            key_hashes: BTreeSet::from([def.secrets.agent_key_hash.as_str()]),
            agents: existing_agent(&def.provider).into_iter().collect(),
        };
        for (index, consumer) in self.consumers.iter().enumerate() {
            validate_consumer(def, consumer, index, &bots, &known, &mut seen)?;
        }
        validate_consumer_keys(def, &self.consumers)?;
        if !(1..=MAX_TARGETS).contains(&self.max_targets) {
            return Err(ValidationError::new(
                "/routing/max_targets",
                "must be 1 to 8",
            ));
        }
        let ready: BTreeSet<&ConsumerId> = self
            .consumers
            .iter()
            .filter(|consumer| !consumer.pending)
            .map(|consumer| &consumer.id)
            .collect();
        if !self.valid_targets(&self.default_targets, |id| {
            id.is_default() || ready.contains(id)
        }) {
            return Err(ValidationError::new(
                "/routing/default_targets",
                "must name 1 to max_targets ready consumers",
            ));
        }
        self.validate_rules(&known)?;
        if let Some(reply) = &self.default_reply {
            validate_default_reply(def, reply)?;
        }
        validate_agent_events(&self.agent_events)
    }

    /// 1..=max_targets unique targets, each accepted by `allowed`.
    fn valid_targets(&self, targets: &[ConsumerId], allowed: impl Fn(&ConsumerId) -> bool) -> bool {
        let unique: BTreeSet<_> = targets.iter().collect();
        !targets.is_empty()
            && targets.len() <= usize::from(self.max_targets)
            && unique.len() == targets.len()
            && targets.iter().all(allowed)
    }

    fn validate_rules(&self, known: &BTreeSet<&ConsumerId>) -> Result<(), ValidationError> {
        if self.rules.len() > MAX_RULES {
            return Err(ValidationError::new("/routing/rules", "at most 32 rules"));
        }
        let mut ids = BTreeSet::new();
        for (index, rule) in self.rules.iter().enumerate() {
            let pointer = format!("/routing/rules/{index}");
            if !ids.insert(&rule.id) {
                return Err(ValidationError::new(
                    format!("{pointer}/id"),
                    "must be a unique rule id",
                ));
            }
            validate_match(&rule.when, &format!("{pointer}/when"))?;
            if !self.valid_targets(&rule.targets, |id| id.is_default() || known.contains(id)) {
                return Err(ValidationError::new(
                    format!("{pointer}/targets"),
                    "must name 1 to max_targets known consumers",
                ));
            }
        }
        Ok(())
    }
}

fn validate_consumer<'a>(
    def: &ChannelDefinition,
    consumer: &'a Consumer,
    index: usize,
    bots: &[String],
    known: &BTreeSet<&ConsumerId>,
    seen: &mut Seen<'a>,
) -> Result<(), ValidationError> {
    let pointer = format!("/routing/consumers/{index}");
    if consumer.id.is_default() || !seen.ids.insert(&consumer.id) {
        return Err(ValidationError::new(format!("{pointer}/id"), UNIQUE_ID));
    }
    if consumer.name.is_default() || !seen.names.insert(consumer.name.as_str()) {
        return Err(ValidationError::new(format!("{pointer}/name"), UNIQUE_NAME));
    }
    validate_name(&consumer.name, bots, &format!("{pointer}/name"))?;
    validate_label(consumer.label.as_deref(), &format!("{pointer}/label"))?;
    consumer.profile.validate().map_err(|error| {
        ValidationError::new(format!("{pointer}/profile{}", error.pointer), error.message)
    })?;
    if consumer.profile_digest != consumer.profile.digest() {
        return Err(ValidationError::new(
            format!("{pointer}/profile_digest"),
            "does not match profile",
        ));
    }
    if consumer.profile.metadata.provider != consumer.provider.slug {
        return Err(ValidationError::new(
            format!("{pointer}/provider/slug"),
            "must match the profile provider",
        ));
    }
    consumer.provider.target.validate().map_err(|error| {
        ValidationError::new(format!("{pointer}{}", error.pointer), error.message)
    })?;
    let hash_pointer = format!("{pointer}/agent_key_hash");
    validate_hash(&consumer.agent_key_hash, &hash_pointer)?;
    if !seen.key_hashes.insert(&consumer.agent_key_hash) {
        return Err(ValidationError::new(hash_pointer, KEY_REUSED));
    }
    if let Some(agent) = existing_agent(&consumer.provider) {
        if !seen.agents.insert(agent) {
            return Err(ValidationError::new(
                format!("{pointer}/provider/target"),
                DUPLICATE_AGENT_TARGET,
            ));
        }
    }
    validate_policy(
        def,
        &pointer,
        Some(&consumer.id),
        &consumer.provider,
        &consumer.reply,
        &consumer.publish,
        known,
    )?;
    OffsetDateTime::parse(&consumer.created_at, &Rfc3339)
        .map_err(|_| ValidationError::new(format!("{pointer}/created_at"), "must be RFC 3339"))?;
    Ok(())
}

/// A name no directive reads otherwise: not reserved, and not a source's bot
/// username, which a leading `@bot` already addresses.
fn validate_name(
    name: &ConsumerName,
    bots: &[String],
    pointer: &str,
) -> Result<(), ValidationError> {
    // Names are lowercase by grammar, so lowering the bots compares case-insensitively.
    if name.is_reserved() || bots.iter().any(|bot| bot == name.as_str()) {
        return Err(ValidationError::new(pointer, UNIQUE_NAME));
    }
    Ok(())
}

fn validate_label(label: Option<&str>, pointer: &str) -> Result<(), ValidationError> {
    let Some(label) = label else {
        return Ok(());
    };
    if !(1..=MAX_LABEL_CHARS).contains(&label.chars().count())
        || label
            .chars()
            .any(|character| character.is_control() || is_bidi_control(character))
    {
        return Err(ValidationError::new(pointer, LABEL));
    }
    Ok(())
}

/// Characters that reorder the text around them, so a label could
/// impersonate another consumer's prefix.
fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

fn existing_agent(provider: &ProviderBinding) -> Option<&str> {
    match &provider.target {
        ProviderTarget::ExistingAgent { agent_id } => Some(agent_id),
        ProviderTarget::Profile {} => None,
    }
}

/// The reply and publish rules of one consumer, `own` being `None` for
/// `default`. Existing agents never receive gateway tools, so they can only
/// answer through automatic finalization and can never publish.
fn validate_policy(
    def: &ChannelDefinition,
    pointer: &str,
    own: Option<&ConsumerId>,
    provider: &ProviderBinding,
    reply: &ConsumerReply,
    publish: &PublishPolicy,
    known: &BTreeSet<&ConsumerId>,
) -> Result<(), ValidationError> {
    let borrowed = existing_agent(provider).is_some();
    if borrowed && !reply.auto_final.unwrap_or(def.reply.auto_final) {
        return Err(ValidationError::new(
            format!("{pointer}/reply/auto_final"),
            "existing agents require auto_final",
        ));
    }
    if borrowed && publish.enabled {
        return Err(ValidationError::new(
            format!("{pointer}/publish/enabled"),
            "publishing requires a profile target",
        ));
    }
    for (index, target) in publish.targets.iter().enumerate() {
        let other = match own {
            None => !target.is_default() && known.contains(target),
            Some(own) => target != own && (target.is_default() || known.contains(target)),
        };
        if !other {
            return Err(ValidationError::new(
                format!("{pointer}/publish/targets/{index}"),
                "must name another consumer",
            ));
        }
    }
    Ok(())
}

/// Exactly one bounded bearer key per additional consumer, and no other.
fn validate_consumer_keys(
    def: &ChannelDefinition,
    consumers: &[Consumer],
) -> Result<(), ValidationError> {
    let keys = &def.secrets.consumer_keys;
    let stored: BTreeSet<&ConsumerId> = keys.iter().map(|(id, _)| id).collect();
    let expected: BTreeSet<&ConsumerId> = consumers.iter().map(|consumer| &consumer.id).collect();
    let bounded = keys.iter().all(|(_, key)| {
        (1..=MAX_CONSUMER_KEY_BYTES).contains(&key.as_str().len())
            && key.as_str().bytes().all(|byte| (33..=126).contains(&byte))
    });
    if stored.len() != keys.len() || stored != expected || !bounded {
        return Err(ValidationError::new(
            "/secrets/consumer_keys",
            ONE_KEY_PER_CONSUMER,
        ));
    }
    Ok(())
}

fn validate_match(when: &RuleMatch, pointer: &str) -> Result<(), ValidationError> {
    for (field, length) in [
        ("sources", when.sources.len()),
        ("kinds", when.kinds.len()),
        ("chats", when.chats.len()),
        ("senders", when.senders.len()),
        ("event_types", when.event_types.len()),
        ("event_sources", when.event_sources.len()),
    ] {
        if length > MAX_MATCH_ENTRIES {
            return Err(ValidationError::new(
                format!("{pointer}/{field}"),
                "must contain at most 64 entries",
            ));
        }
    }
    // Sources and kinds are closed vocabularies and patterns bound themselves.
    for (field, values) in [("chats", &when.chats), ("senders", &when.senders)] {
        for (index, value) in values.iter().enumerate() {
            require_string(
                value,
                &format!("{pointer}/{field}/{index}"),
                1,
                MAX_MATCH_ENTRY_BYTES,
            )?;
        }
    }
    Ok(())
}

fn validate_default_reply(
    def: &ChannelDefinition,
    reply: &DefaultReply,
) -> Result<(), ValidationError> {
    const POINTER: &str = "/routing/default_reply";
    let attached = |source_id: &super::SourceId| {
        def.sources
            .iter()
            .find(|source| &source.source_id() == source_id)
    };
    let unattached = || {
        ValidationError::new(
            format!("{POINTER}/source_id"),
            "must name an attached source of this type",
        )
    };
    match reply {
        DefaultReply::TelegramChat {
            source_id,
            chat_id,
            thread_id,
        } => {
            if !matches!(attached(source_id), Some(Source::Telegram { .. })) {
                return Err(unattached());
            }
            if canonical_integer(chat_id).is_none_or(|id| id == 0) {
                return Err(ValidationError::new(
                    format!("{POINTER}/chat_id"),
                    "must be a non-zero integer",
                ));
            }
            if thread_id
                .as_deref()
                .is_some_and(|id| canonical_integer(id).is_none_or(|id| id <= 0))
            {
                return Err(ValidationError::new(
                    format!("{POINTER}/thread_id"),
                    "must be a positive integer",
                ));
            }
        }
        DefaultReply::RelayConversation {
            source_id,
            route_id,
        } => {
            let Some(Source::NyxidRelay { route_ids, .. }) = attached(source_id) else {
                return Err(unattached());
            };
            // NyxID's channel-relay send addresses a route by UUID.
            if route_id.len() != 36 || uuid::Uuid::try_parse(route_id).is_err() {
                return Err(ValidationError::new(
                    format!("{POINTER}/route_id"),
                    "must be a UUID",
                ));
            }
            if !route_ids.iter().any(|route| route == route_id) {
                return Err(ValidationError::new(
                    format!("{POINTER}/route_id"),
                    "must be a route of that relay source",
                ));
            }
        }
        DefaultReply::Sinks {} => {
            if !def
                .reply
                .sinks
                .iter()
                .any(|sink| matches!(sink, Sink::ResultWebhook { .. }))
            {
                return Err(ValidationError::new(
                    POINTER,
                    "requires a result_webhook sink",
                ));
            }
        }
    }
    Ok(())
}

/// An i64 in its canonical decimal spelling.
fn canonical_integer(value: &str) -> Option<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| id.to_string() == value)
}

fn validate_agent_events(policy: &AgentEventPolicy) -> Result<(), ValidationError> {
    const POINTER: &str = "/routing/agent_events";
    for (field, value, max) in [
        ("max_hops", u16::from(policy.max_hops), 4),
        (
            "per_consumer_per_minute",
            policy.per_consumer_per_minute,
            30,
        ),
        ("per_channel_per_minute", policy.per_channel_per_minute, 60),
    ] {
        if !(1..=max).contains(&value) {
            return Err(ValidationError::new(
                format!("{POINTER}/{field}"),
                format!("must be 1 to {max}"),
            ));
        }
    }
    if policy.per_consumer_per_minute > policy.per_channel_per_minute {
        return Err(ValidationError::new(
            format!("{POINTER}/per_consumer_per_minute"),
            "must not exceed per_channel_per_minute",
        ));
    }
    Ok(())
}
