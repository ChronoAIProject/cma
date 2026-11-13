//! Compact binary fields keep legal source rows below SQLite's existing value
//! limit. Earlier held-checkpoint numeric arrays remain readable without rewrite.
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const LIMIT: usize = crate::mutation_journal::MAX_JOURNAL_BYTES as usize;

#[derive(Deserialize)]
#[serde(untagged)]
enum Input {
    Compact(String),
    Legacy(Vec<u8>),
}

impl Input {
    fn decode<E: serde::de::Error>(self) -> Result<Vec<u8>, E> {
        let bytes = match self {
            Self::Compact(value) => {
                if value.len() > LIMIT.div_ceil(3) * 4 {
                    return Err(E::custom("oversized archive binary field"));
                }
                STANDARD
                    .decode(value)
                    .map_err(|_| E::custom("invalid archive binary field"))?
            }
            Self::Legacy(bytes) => bytes,
        };
        if bytes.len() > LIMIT {
            return Err(E::custom("oversized archive binary field"));
        }
        Ok(bytes)
    }
}

pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&STANDARD.encode(bytes))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    Input::deserialize(deserializer)?.decode()
}

pub(super) mod optional {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        bytes
            .as_ref()
            .map(|bytes| STANDARD.encode(bytes))
            .serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<Input>::deserialize(deserializer)?
            .map(Input::decode)
            .transpose()
    }
}

pub(super) mod folded {
    use super::*;
    pub(crate) fn serialize<S: Serializer>(
        bytes: &[Option<Vec<u8>>; 3],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        bytes
            .each_ref()
            .map(|bytes| bytes.as_ref().map(|bytes| STANDARD.encode(bytes)))
            .serialize(serializer)
    }
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[Option<Vec<u8>>; 3], D::Error> {
        let [first, second, third] = <[Option<Input>; 3]>::deserialize(deserializer)?
            .map(|field| field.map(Input::decode).transpose());
        Ok([first?, second?, third?])
    }
}
