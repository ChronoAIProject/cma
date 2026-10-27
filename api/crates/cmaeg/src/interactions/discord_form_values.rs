//! Strict whole-modal decoding; never trust callback field names as business IDs.
use super::{discord_form::schema, lark_form::Submitted, Error};
use cmaeg_core::message::{FormFieldKind, OutboundMessage};
use serde_json::Value;
use std::collections::BTreeMap;
const MAX_BYTES: usize = 64 * 1024;

pub(crate) fn submitted(message: &OutboundMessage, components: &Value) -> Result<Submitted, Error> {
    let (_, _, fields) = schema(message)?;
    let components = components
        .as_array()
        .filter(|values| values.len() <= fields.len())
        .ok_or(Error::Denied)?;
    if serde_json::to_vec(components)
        .map_err(|_| Error::Denied)?
        .len()
        > MAX_BYTES
    {
        return Err(Error::Denied);
    }
    let mut seen = BTreeMap::new();
    for label in components {
        if label["type"] != 18 {
            return Err(Error::Denied);
        }
        let child = &label["component"];
        let name = child["custom_id"].as_str().ok_or(Error::Denied)?;
        let index = name
            .strip_prefix('f')
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or(Error::Denied)?;
        if name != format!("f{index}")
            || index >= fields.len()
            || seen.insert(index, child).is_some()
        {
            return Err(Error::Denied);
        }
    }
    let mut result = Submitted::new();
    for (index, field) in fields.iter().enumerate() {
        let value = match seen.get(&index) {
            None if !field.required => continue,
            None => return Err(Error::Denied),
            Some(child) => match &field.kind {
                FormFieldKind::Choice { options } => {
                    if child["type"] != 3 || child.get("value").is_some() {
                        return Err(Error::Denied);
                    }
                    let values = child["values"].as_array().ok_or(Error::Denied)?;
                    if values.is_empty() && !field.required {
                        continue;
                    }
                    if values.len() != 1 {
                        return Err(Error::Denied);
                    }
                    let value = values[0].as_str().ok_or(Error::Denied)?;
                    let i = value.parse::<usize>().map_err(|_| Error::Denied)?;
                    if i.to_string() != value {
                        return Err(Error::Denied);
                    }
                    Value::String(options.get(i).ok_or(Error::Denied)?.id.clone())
                }
                kind => {
                    if child["type"] != 4 || child.get("values").is_some() {
                        return Err(Error::Denied);
                    }
                    let value = child["value"].as_str().ok_or(Error::Denied)?;
                    if value.is_empty() && !field.required {
                        continue;
                    }
                    parse(kind, value)?
                }
            },
        };
        result.insert(field.id.clone(), value);
    }
    valid(message, &result)
        .then_some(result)
        .ok_or(Error::Denied)
}
fn parse(kind: &FormFieldKind, value: &str) -> Result<Value, Error> {
    if value.is_empty() {
        return Err(Error::Denied);
    }
    match kind {
        FormFieldKind::Text {} if value.len() <= 4000 && value.chars().count() <= 1000 => {
            Ok(value.into())
        }
        FormFieldKind::Number {} if value.len() <= 64 && value.trim() == value => {
            serde_json::from_str::<serde_json::Number>(value)
                .map(Value::Number)
                .map_err(|_| Error::Denied)
        }
        FormFieldKind::Date {} if value.len() == 10 => {
            let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
                .map_err(|_| Error::Denied)?;
            let date = time::Date::parse(value, &format).map_err(|_| Error::Denied)?;
            if date.to_string() != value {
                return Err(Error::Denied);
            }
            Ok(value.into())
        }
        _ => Err(Error::Denied),
    }
}
pub(crate) fn valid(message: &OutboundMessage, values: &Submitted) -> bool {
    let Ok((_, _, fields)) = schema(message) else {
        return false;
    };
    values.len() <= fields.len()
        && serde_json::to_vec(values).is_ok_and(|bytes| bytes.len() <= MAX_BYTES)
        && values
            .keys()
            .all(|key| fields.iter().any(|field| &field.id == key))
        && fields.iter().all(|field| match values.get(&field.id) {
            None => !field.required,
            Some(value) => match &field.kind {
                FormFieldKind::Choice { options } => value
                    .as_str()
                    .is_some_and(|id| options.iter().any(|option| option.id == id)),
                FormFieldKind::Number {} => value.is_number(),
                kind => value
                    .as_str()
                    .is_some_and(|value| parse(kind, value).is_ok()),
            },
        })
}
