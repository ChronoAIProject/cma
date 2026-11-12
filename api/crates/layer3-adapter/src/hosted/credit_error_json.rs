//! Read the two credit-error discriminators without owning an embedded JSON tree.

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::fmt;

pub(super) fn matches(body: &str) -> bool {
    if body.len() > 4 * 1024 * 1024 {
        return false;
    }
    // Transport diagnostics may follow the first complete JSON value.
    serde_json::Deserializer::from_str(body)
        .into_iter::<CreditError>()
        .next()
        .is_some_and(|value| value.is_ok_and(|value| value.0))
}

struct CreditError(bool);

impl<'de> Deserialize<'de> for CreditError {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = CreditError;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a credit error object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let (mut error, mut code) = (false, false);
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        // Match Value's existing last-key-wins behavior.
                        Field::Error => error = map.next_value_seed(Match(Field::Error))?,
                        Field::Code => code = map.next_value_seed(Match(Field::Code))?,
                        Field::Other => {
                            map.next_value::<Skip>()?;
                        }
                    }
                }
                Ok(CreditError(error && code))
            }
        }
        decoder.deserialize_map(Fields)
    }
}

#[derive(Clone, Copy)]
enum Field {
    Error,
    Code,
    Other,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct Key;
        impl Visitor<'_> for Key {
            type Value = Field;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an error field name")
            }

            fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Field, E> {
                Ok(match key {
                    "error" => Field::Error,
                    "error_code" => Field::Code,
                    _ => Field::Other,
                })
            }
        }
        decoder.deserialize_identifier(Key)
    }
}

struct Match(Field);

impl<'de> DeserializeSeed<'de> for Match {
    type Value = bool;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<bool, D::Error> {
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Match {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an error discriminator")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<bool, E> {
        Ok(matches!(self.0, Field::Error) && value == "insufficient_credits")
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<bool, E> {
        Ok(matches!(self.0, Field::Code) && value == 11300)
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<bool, E> {
        Ok(matches!(self.0, Field::Code) && value == 11300)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut values: S) -> Result<bool, S::Error> {
        while values.next_element::<Skip>()?.is_some() {}
        Ok(false)
    }

    fn visit_map<M: MapAccess<'de>>(self, mut fields: M) -> Result<bool, M::Error> {
        while fields.next_entry::<Skip, Skip>()?.is_some() {}
        Ok(false)
    }
}

// Descend through the normal parser so its recursion limit remains effective.
// serde_json's special IgnoredAny scanner does not use that recursion budget.
struct Skip;

impl<'de> Deserialize<'de> for Skip {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_any(Skip)
    }
}

impl<'de> Visitor<'de> for Skip {
    type Value = Skip;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ignored JSON value")
    }

    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut values: S) -> Result<Skip, S::Error> {
        while values.next_element::<Skip>()?.is_some() {}
        Ok(Skip)
    }

    fn visit_map<M: MapAccess<'de>>(self, mut fields: M) -> Result<Skip, M::Error> {
        while fields.next_entry::<Skip, Skip>()?.is_some() {}
        Ok(Skip)
    }
}

#[cfg(test)]
#[path = "credit_error_json_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "credit_error_json_allocation_tests.rs"]
mod allocation_tests;
