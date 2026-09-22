//! Opaque message identifiers, including stateless sealed delivery references.
use crate::{
    seal::{
        open, seal, MasterKeyRing, MessageRefPayload, OpenError, Opened, SealError, TokenKind,
        MAX_TOKEN_LENGTH,
    },
    ValidationError,
};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
use utoipa::ToSchema;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, ToSchema)]
#[schema(value_type=String)]
pub struct MessageRef(String);
impl MessageRef {
    /// Plain inbound message IDs retain their original bound; sealed refs allow 4 KiB.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let limit = if value.starts_with("cmaeg1.mr.") {
            MAX_TOKEN_LENGTH
        } else {
            super::MAX_ID_BYTES
        };
        if value.is_empty() || value.len() > limit {
            return Err(ValidationError::new(
                "/",
                "message reference has an invalid length",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn seal(payload: &MessageRefPayload, keys: &MasterKeyRing) -> Result<Self, SealError> {
        seal(keys, TokenKind::MessageRef, payload).map(Self)
    }
    pub fn open(&self, keys: &MasterKeyRing) -> Result<Opened<MessageRefPayload>, OpenError> {
        let value: Opened<MessageRefPayload> = open(keys, TokenKind::MessageRef, &self.0)?;
        value.payload.validate().map_err(|_| OpenError::Format)?;
        Ok(value)
    }
}
impl fmt::Debug for MessageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted message ref]")
    }
}
impl fmt::Display for MessageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl FromStr for MessageRef {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}
impl Serialize for MessageRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for MessageRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}
