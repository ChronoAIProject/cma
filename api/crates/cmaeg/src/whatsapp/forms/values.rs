//! Bounded flat completion parsing rejects duplicate keys before map projection.
use super::{compile::schema, Error, MAX_SUBMITTED_BYTES};
use cmaeg_core::message::{FormFieldKind, OutboundMessage};
use serde::{
    de::{Error as _, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

pub(crate) type Submitted = BTreeMap<String, Value>;

struct Flat(Submitted);
impl<'de> Deserialize<'de> for Flat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FlatVisitor;
        impl<'de> Visitor<'de> for FlatVisitor {
            type Value = Flat;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("one bounded flat object with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Flat, M::Error> {
                let mut result = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if result.len() >= 13
                        || key.len() > 16
                        || result.contains_key(&key)
                        || !matches!(&value, Value::String(_) | Value::Null)
                    {
                        return Err(M::Error::custom("invalid flat Flow response"));
                    }
                    result.insert(key, value);
                }
                Ok(Flat(result))
            }
        }
        deserializer.deserialize_map(FlatVisitor)
    }
}

pub(crate) fn decode(raw: &str) -> Result<(String, Submitted), Error> {
    if raw.len() > MAX_SUBMITTED_BYTES {
        return Err(Error::Invalid);
    }
    let Flat(mut fields) = serde_json::from_str(raw).map_err(|_| Error::Invalid)?;
    let Value::String(token) = fields.remove("flow_token").ok_or(Error::Invalid)? else {
        return Err(Error::Invalid);
    };
    if token.is_empty() || token.len() > 128 {
        return Err(Error::Invalid);
    }
    Ok((token, fields))
}

pub(crate) fn submitted(message: &OutboundMessage, input: &Submitted) -> Result<Submitted, Error> {
    let (_, _, fields) = schema(message)?;
    if input.len() > fields.len()
        || serde_json::to_vec(input).map_or(true, |v| v.len() > MAX_SUBMITTED_BYTES)
    {
        return Err(Error::Invalid);
    }
    for name in input.keys() {
        let suffix = name.strip_prefix('f').ok_or(Error::Invalid)?;
        let index = suffix.parse::<usize>().map_err(|_| Error::Invalid)?;
        if index >= fields.len() || suffix != index.to_string() {
            return Err(Error::Invalid);
        }
    }
    let mut result = BTreeMap::new();
    for (i, field) in fields.iter().enumerate() {
        let raw = input.get(&format!("f{i}"));
        if raw.is_none() || raw == Some(&Value::Null) || raw == Some(&Value::String(String::new()))
        {
            if field.required {
                return Err(Error::Invalid);
            }
            continue;
        }
        let value = raw.and_then(Value::as_str).ok_or(Error::Invalid)?;
        let typed = match &field.kind {
            FormFieldKind::Text {} if value.len() <= 4000 && value.chars().count() <= 1000 => {
                value.into()
            }
            FormFieldKind::Number {} if value.len() <= 64 && value.trim() == value => {
                let number: serde_json::Number =
                    serde_json::from_str(value).map_err(|_| Error::Invalid)?;
                Value::Number(number)
            }
            FormFieldKind::Date {} if valid_date(value) => value.into(),
            FormFieldKind::Choice { options } => {
                let index = value.parse::<usize>().map_err(|_| Error::Invalid)?;
                if value != index.to_string() {
                    return Err(Error::Invalid);
                }
                options.get(index).ok_or(Error::Invalid)?.id.clone().into()
            }
            _ => return Err(Error::Invalid),
        };
        result.insert(field.id.clone(), typed);
    }
    Ok(result)
}

fn valid_date(value: &str) -> bool {
    if value.len() != 10 || !value.is_ascii() {
        return false;
    }
    let Ok(format) = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]") else {
        return false;
    };
    time::Date::parse(value, &format).is_ok_and(|date| date.to_string() == value)
}
