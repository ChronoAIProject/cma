//! Borrow admitted values without copying untrusted fields into Serde diagnostics.

use std::fmt;

use serde::de::value::{
    BorrowedStrDeserializer, MapAccessDeserializer, MapDeserializer, SeqDeserializer,
};
use serde::de::{DeserializeOwned, IntoDeserializer, Visitor};
use serde::{forward_to_deserialize_any, Deserializer};
use serde_json::Value;

#[derive(Debug)]
pub(super) enum Error {
    Legacy(serde_json::Error),
    Bounded,
}

impl fmt::Display for Error {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(error) => fmt::Display::fmt(error, output),
            Self::Bounded => output.write_str("runtime value does not match the supported schema"),
        }
    }
}

impl std::error::Error for Error {}

impl serde::de::Error for Error {
    fn custom<T: fmt::Display>(_: T) -> Self {
        // Do not format the supplied value: it may contain the entire frame.
        Self::Bounded
    }
}

pub(super) fn decode<T: DeserializeOwned>(value: &Value, bounded: bool) -> Result<T, Error> {
    if bounded {
        T::deserialize(Borrowed(value))
    } else {
        T::deserialize(value).map_err(Error::Legacy)
    }
}

#[derive(Clone, Copy)]
struct Borrowed<'de>(&'de Value);

impl<'de> IntoDeserializer<'de, Error> for Borrowed<'de> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

impl<'de> Deserializer<'de> for Borrowed<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            Value::Bool(value) => visitor.visit_bool(*value),
            Value::Number(value) => {
                if let Some(value) = value.as_u64() {
                    visitor.visit_u64(value)
                } else if let Some(value) = value.as_i64() {
                    visitor.visit_i64(value)
                } else {
                    visitor.visit_f64(value.as_f64().ok_or(Error::Bounded)?)
                }
            }
            Value::String(value) => visitor.visit_borrowed_str(value),
            Value::Array(values) => {
                SeqDeserializer::new(values.iter().map(Borrowed)).deserialize_any(visitor)
            }
            Value::Object(values) => {
                MapDeserializer::new(values.iter().map(|(key, value)| {
                    (BorrowedStrDeserializer::new(key.as_str()), Borrowed(value))
                }))
                .deserialize_any(visitor)
            }
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::String(value) => visitor.visit_borrowed_str(value),
            _ => Err(Error::Bounded),
        }
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            Value::String(value) => BorrowedStrDeserializer::new(value.as_str())
                .deserialize_enum(name, variants, visitor),
            Value::Object(values) if values.len() == 1 => {
                let map = MapDeserializer::new(values.iter().map(|(key, value)| {
                    (BorrowedStrDeserializer::new(key.as_str()), Borrowed(value))
                }));
                MapAccessDeserializer::new(map).deserialize_enum(name, variants, visitor)
            }
            _ => Err(Error::Bounded),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct
    }
}

#[cfg(test)]
#[path = "runtime_decode_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "runtime_decode_allocation_tests.rs"]
mod allocation_tests;
