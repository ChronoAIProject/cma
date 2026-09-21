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

id_type!(ChannelId);
id_type!(RecordId);
id_type!(EventId);
id_type!(ConversationId);
id_type!(MessageRef);
id_type!(BindingId);
id_type!(ProviderSlug);

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
}
