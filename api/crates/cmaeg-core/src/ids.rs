//! Opaque identifiers carried by the CMAEG protocol.

use std::{fmt, str::FromStr};

use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;

use crate::ValidationError;

/// The largest identifier accepted at a public boundary.
pub const MAX_ID_BYTES: usize = 512;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, ToSchema)]
        #[schema(value_type = String)]
        pub struct $name(String);

        impl $name {
            /// Construct an identifier after applying the wire-size bound.
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let value = value.into();
                if value.is_empty() {
                    return Err(ValidationError::new("/", "identifier must not be empty"));
                }
                if value.len() > MAX_ID_BYTES {
                    return Err(ValidationError::new(
                        "/",
                        format!("identifier exceeds {MAX_ID_BYTES} bytes"),
                    ));
                }
                Ok(Self(value))
            }

            /// Borrow the opaque identifier.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ValidationError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(D::Error::custom)
            }
        }
    };
}

id_type!(RecordId);
id_type!(EventId);
id_type!(ConversationId);
mod message_ref;
pub use message_ref::MessageRef;
id_type!(BindingId);
id_type!(ProviderSlug);
mod consumer;
pub use consumer::{
    compact_partition_key, ConsumerId, ConsumerName, RuleId, DEFAULT_CONSUMER_ID,
    MAX_SEALED_PARTITION_KEY_BYTES,
};

/// An encrypted pointer to a CMA channel record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, ToSchema)]
#[schema(value_type = String)]
pub struct ChannelId(String);

impl ChannelId {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if !value.starts_with("cmaeg1.ch.") || value.len() > MAX_ID_BYTES {
            return Err(ValidationError::new(
                "/",
                "channel id has an invalid prefix or length",
            ));
        }
        Ok(Self(value))
    }

    pub(crate) fn from_sealed(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn open(
        &self,
        ring: &crate::seal::MasterKeyRing,
    ) -> Result<crate::seal::Opened<crate::seal::ChannelPointer>, crate::seal::OpenError> {
        crate::seal::ChannelPointer::open_id(self, ring)
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ChannelId {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for ChannelId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ChannelId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl ConversationId {
    /// Whether this value has the deterministic provider conversation shape.
    pub fn is_wellformed(&self) -> bool {
        let suffix = self.as_str().strip_prefix("conv_");
        suffix.is_some_and(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }

    /// Derive a stable provider conversation id using the ring's derivation key.
    pub fn derive(
        ring: &crate::seal::MasterKeyRing,
        channel_id: &ChannelId,
        rule_version: u16,
        partition_key: &str,
    ) -> Self {
        Self::derive_with_kid(
            ring,
            ring.derivation_kid(),
            channel_id,
            rule_version,
            partition_key,
        )
    }

    /// Derive with an explicitly retained key during a channel rekey walk.
    pub fn derive_with_kid(
        ring: &crate::seal::MasterKeyRing,
        kid: u8,
        channel_id: &ChannelId,
        rule_version: u16,
        partition_key: &str,
    ) -> Self {
        let mut input = Vec::new();
        input.extend_from_slice(b"cmaeg.conversation.v1\x1f");
        input.extend_from_slice(channel_id.as_str().as_bytes());
        input.extend_from_slice(b"\x1f");
        input.extend_from_slice(rule_version.to_string().as_bytes());
        input.extend_from_slice(b"\x1f");
        input.extend_from_slice(partition_key.as_bytes());
        let signature = crate::seal::sign(ring, kid, &input);
        let value = format!("conv_{}", hex(&signature[..16]));
        Self(value)
    }
}

/// An idempotency key that is stable for one inbound event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema)]
#[schema(value_type = String)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Construct a bounded printable provider request key.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 200
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(ValidationError::new(
                "/idempotency_key",
                "must be 1..=200 safe ASCII bytes",
            ));
        }
        Ok(Self(value))
    }

    pub fn from_event(event_id: &EventId) -> Self {
        let mut value = String::from("evt_");
        value.extend(event_id.as_str().chars().map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        }));
        value.truncate(200);
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                HEX[(byte >> 4) as usize] as char,
                HEX[(byte & 0xf) as usize] as char,
            ]
        })
        .collect()
}
