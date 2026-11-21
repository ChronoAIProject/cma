//! Identifiers of the agents that consume a channel's events, their names,
//! the routing rules that select them, and the conversation and idempotency
//! identities derived for each of them.
use crate::ValidationError;
use serde::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// The identifier of the implicit consumer every channel has: the provider
/// binding, profile and agent key stored at the top level of its definition.
pub const DEFAULT_CONSUMER_ID: &str = "default";
const GENERATED_PREFIX: &str = "csm_";
const GENERATED_HEX_DIGITS: usize = 12;
/// Words a chat directive reads as an audience or as itself, never as one consumer.
const RESERVED_NAMES: [&str; 4] = ["all", "everyone", "here", "to"];
const MAX_NAME_BYTES: usize = 32;
const MAX_RULE_ID_BYTES: usize = 32;

/// A channel consumer's stable identifier: "default" or "csm_" + 12 lowercase hex.
/// Assigned by the gateway and never reused within a channel.
///
/// The grammar admits no `\x1f`, the separator of the gateway's identity
/// derivations, so an identifier can be joined into one unambiguously.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^(default|csm_[0-9a-f]{12})$")]
pub struct ConsumerId(String);

impl ConsumerId {
    /// The implicit consumer backed by the definition's top-level fields.
    pub fn default_consumer() -> Self {
        Self(DEFAULT_CONSUMER_ID.to_owned())
    }

    /// Accept `default` or a gateway-assigned `csm_` identifier, exactly.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let generated = value.strip_prefix(GENERATED_PREFIX).is_some_and(|digits| {
            digits.len() == GENERATED_HEX_DIGITS
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
        if value != DEFAULT_CONSUMER_ID && !generated {
            return Err(ValidationError::new(
                "/",
                "consumer id must be \"default\" or \"csm_\" followed by 12 lowercase hex digits",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_CONSUMER_ID
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ConsumerId {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ConsumerId> for String {
    fn from(id: ConsumerId) -> Self {
        id.0
    }
}

impl fmt::Display for ConsumerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A consumer's chat name, `^[a-z][a-z0-9_-]{1,31}$`, unique per channel.
/// Names are lowercase, so comparing two of them is exact.
///
/// Reserved: `all`, `everyone`, `here`, `to`, any `csm_` prefix, and
/// `default`, which only the default consumer may use. The grammar is a
/// property of the type; the reserved words are refused by the channel
/// readers, which know whether a name belongs to the default consumer and
/// can point at the offending field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^[a-z][a-z0-9_-]{1,31}$")]
pub struct ConsumerName(String);

impl ConsumerName {
    /// Accept exactly the grammar above.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let mut bytes = value.bytes();
        let valid = (2..=MAX_NAME_BYTES).contains(&value.len())
            && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
            && bytes.all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !valid {
            return Err(ValidationError::new(
                "/",
                "consumer name must be a lowercase letter followed by 1 to 31 lowercase letters, digits, '_' or '-'",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Whether no consumer may take this name: a chat audience word, a
    /// directive, or the shape of a generated consumer id.
    pub fn is_reserved(&self) -> bool {
        RESERVED_NAMES.contains(&self.0.as_str()) || self.0.starts_with(GENERATED_PREFIX)
    }

    /// Whether this is `default`, the name reserved for the default consumer.
    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_CONSUMER_ID
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ConsumerName {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ConsumerName> for String {
    fn from(name: ConsumerName) -> Self {
        name.0
    }
}

impl fmt::Display for ConsumerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A routing rule's identifier, `^[a-z0-9_-]{1,32}$`, unique per channel.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String, pattern = "^[a-z0-9_-]{1,32}$")]
pub struct RuleId(String);

impl RuleId {
    /// Accept exactly the grammar above.
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let valid = (1..=MAX_RULE_ID_BYTES).contains(&value.len())
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !valid {
            return Err(ValidationError::new(
                "/",
                "rule id must contain 1 to 32 lowercase letters, digits, '_' or '-'",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RuleId {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RuleId> for String {
    fn from(id: RuleId) -> Self {
        id.0
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The largest partition key an event_ref seals verbatim.
pub const MAX_SEALED_PARTITION_KEY_BYTES: usize = 512;

/// A partition key in the form an event_ref seals it: the raw key when it is
/// at most [`MAX_SEALED_PARTITION_KEY_BYTES`], otherwise `sha256:` followed by
/// the lowercase hex digest of the raw key. Long custom subjects stay whole in
/// the provider's activity; only this compact form travels in references.
///
/// Ingress, fan-out and hand-offs share this one rule, because a hand-off has
/// only its parent's sealed key and must still derive the same per-consumer
/// conversation as the event that arrived at ingress.
pub fn compact_partition_key(raw: &str) -> String {
    if raw.len() <= MAX_SEALED_PARTITION_KEY_BYTES {
        return raw.to_owned();
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, raw.as_bytes());
    format!("sha256:{}", super::hex(digest.as_ref()))
}

impl super::ConversationId {
    /// Derive a non-default consumer's provider conversation id: `conv_` and
    /// 16 bytes of HMAC over `cmaeg.consumer.conversation.v1`, the channel id,
    /// the partition rule version, the consumer id and the compact partition
    /// key, joined by `\x1f`.
    ///
    /// The key is the compact form of [`compact_partition_key`], so ingress
    /// and a hand-off that holds only the sealed key derive the same id. The
    /// distinct domain string keeps these ids disjoint from the default
    /// consumer's, and the consumer id grammar excludes `\x1f`, so no two
    /// inputs join to the same bytes. The derivation never reads an agent key,
    /// so rotating a consumer's key keeps its live conversations.
    ///
    /// The default consumer never uses this: it keeps
    /// [`ConversationId::derive`](super::ConversationId::derive) over the raw
    /// partition key, so its existing conversations are unchanged (I4).
    pub fn derive_for_consumer(
        ring: &crate::seal::MasterKeyRing,
        channel_id: &super::ChannelId,
        rule_version: u16,
        consumer: &ConsumerId,
        compact_partition_key: &str,
    ) -> Self {
        Self::derive_for_consumer_with_kid(
            ring,
            ring.derivation_kid(),
            channel_id,
            rule_version,
            consumer,
            compact_partition_key,
        )
    }

    /// [`derive_for_consumer`](Self::derive_for_consumer) with an explicitly
    /// retained key, for the channel rekey walk.
    pub fn derive_for_consumer_with_kid(
        ring: &crate::seal::MasterKeyRing,
        kid: u8,
        channel_id: &super::ChannelId,
        rule_version: u16,
        consumer: &ConsumerId,
        compact_partition_key: &str,
    ) -> Self {
        debug_assert!(
            !consumer.is_default(),
            "the default consumer keeps ConversationId::derive over the raw partition key"
        );
        let mut input = Vec::new();
        input.extend_from_slice(b"cmaeg.consumer.conversation.v1\x1f");
        input.extend_from_slice(channel_id.as_str().as_bytes());
        input.extend_from_slice(b"\x1f");
        input.extend_from_slice(rule_version.to_string().as_bytes());
        input.extend_from_slice(b"\x1f");
        input.extend_from_slice(consumer.as_str().as_bytes());
        input.extend_from_slice(b"\x1f");
        input.extend_from_slice(compact_partition_key.as_bytes());
        let signature = crate::seal::sign(ring, kid, &input);
        Self(format!("conv_{}", super::hex(&signature[..16])))
    }
}

/// Hex digits of the event id digest in a per-consumer idempotency key.
const CONSUMER_KEY_DIGEST_HEX: usize = 40;

impl super::IdempotencyKey {
    /// A non-default consumer's idempotency key for one event:
    /// `evc_<consumer>_` and the first 40 hex digits of SHA-256 over the event
    /// id, 61 bytes for a `csm_` id.
    ///
    /// The digest makes the key collision-free for event ids that the 200-byte
    /// `evt_` form would truncate to the same prefix, and keeps it within the
    /// provider's bound whatever the id's length. The default consumer keeps
    /// [`IdempotencyKey::from_event`](super::IdempotencyKey::from_event).
    pub fn for_consumer(event_id: &super::EventId, consumer: &ConsumerId) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, event_id.as_str().as_bytes());
        let digits = super::hex(digest.as_ref());
        Self(format!(
            "evc_{consumer}_{}",
            &digits[..CONSUMER_KEY_DIGEST_HEX]
        ))
    }
}
