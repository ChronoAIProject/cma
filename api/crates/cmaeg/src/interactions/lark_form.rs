//! One bounded native form; provider identifiers never become platform locators.
use super::Error;
use cmaeg_core::message::{Block, FormField, FormFieldKind, OptionStyle, OutboundMessage};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type Submitted = BTreeMap<String, Value>;
pub(crate) const MAX_SUBMITTED_BYTES: usize = 16 * 1024;

pub(crate) fn requested(message: &OutboundMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Form { .. }))
}
/// Native forms have one submit action and no independent button/choice actions.
pub(super) fn schema(message: &OutboundMessage) -> Result<(usize, &str, &[FormField]), Error> {
    message.validate().map_err(|_| Error::Denied)?;
    let mut form = None;
    for (index, block) in message.blocks.iter().enumerate() {
        match block {
            Block::Form { id, fields } if form.is_none() => {
                let mut names = BTreeSet::new();
                for field in fields {
                    if !names.insert(&field.id) {
                        return Err(Error::Denied);
                    }
                    if let FormFieldKind::Choice { options } = &field.kind {
                        let mut ids = BTreeSet::new();
                        if options.is_empty()
                            || options.len() > 20
                            || options.iter().filter(|o| o.selected).count() > 1
                            || options.iter().any(|o| {
                                o.id.is_empty()
                                    || o.id.len() > 512
                                    || !ids.insert(&o.id)
                                    || o.label.is_empty()
                                    || o.label.len() > 200
                                    || o.style != OptionStyle::Default
                            })
                        {
                            return Err(Error::Denied);
                        }
                    }
                }
                form = Some((index, id.as_str(), fields.as_slice()));
            }
            Block::Form { .. } | Block::Buttons { .. } | Block::Choice { .. } => {
                return Err(Error::Denied)
            }
            _ => {}
        }
    }
    form.ok_or(Error::Denied)
}

pub(super) fn render(
    message: &OutboundMessage,
    card: &str,
    revision: u64,
) -> Result<(Value, usize), Error> {
    let (index, _, fields) = schema(message)?;
    let mut elements = Vec::new();
    let mut characters = "Submit".len();
    for (number, field) in fields.iter().enumerate() {
        characters += field.label.chars().count();
        elements.push(json!({"tag":"div","text":{"tag":"plain_text","content":field.label}}));
        let mut element = match &field.kind {
            FormFieldKind::Text {} => {
                json!({"tag":"input","input_type":"multiline_text","max_length":1000})
            }
            FormFieldKind::Number {} => {
                characters += "Enter a number".chars().count();
                json!({"tag":"input","input_type":"text","max_length":64,
                    "placeholder":{"tag":"plain_text","content":"Enter a number"}})
            }
            FormFieldKind::Date {} => json!({"tag":"date_picker"}),
            FormFieldKind::Choice { options } => {
                let choices: Vec<_> = options.iter().enumerate().map(|(i, option)| {
                    characters += option.label.chars().count();
                    json!({"text":{"tag":"plain_text","content":option.label},"value":i.to_string()})
                }).collect();
                let mut value = json!({"tag":"select_static","options":choices});
                if let Some(i) = options.iter().position(|option| option.selected) {
                    value["initial_option"] = i.to_string().into();
                }
                value
            }
        };
        element["name"] = format!("f{number}").into();
        element["required"] = field.required.into();
        elements.push(element);
    }
    elements.push(json!({"tag":"button","type":"primary","name":"submit",
        "text":{"tag":"plain_text","content":"Submit"},"form_action_type":"submit",
        "behaviors":[{"type":"callback","value":{"cma_action":format!("clf1:{}:{revision}:{index}",card.replace('-',""))}}]}));
    Ok((
        json!({"tag":"form","name":"form","elements":elements}),
        characters,
    ))
}

/// Convert authenticated opaque field values into the card's business schema.
pub(super) fn submitted(
    message: &OutboundMessage,
    index: usize,
    input: &serde_json::Map<String, Value>,
) -> Result<Submitted, Error> {
    let (expected, _, fields) = schema(message)?;
    if expected != index
        || input.len() > fields.len()
        || serde_json::to_vec(input).map_err(|_| Error::Denied)?.len() > MAX_SUBMITTED_BYTES
    {
        return Err(Error::Denied);
    }
    for name in input.keys() {
        let suffix = name.strip_prefix('f').ok_or(Error::Denied)?;
        let number = suffix.parse::<usize>().map_err(|_| Error::Denied)?;
        if number >= fields.len() || suffix != number.to_string() {
            return Err(Error::Denied);
        }
    }
    let mut result = BTreeMap::new();
    for (i, field) in fields.iter().enumerate() {
        let raw = input.get(&format!("f{i}"));
        // Providers may omit optional controls or return null/empty strings.
        if raw.is_none_or(|v| v.is_null() || v.as_str() == Some("")) {
            if field.required {
                return Err(Error::Denied);
            }
            continue;
        }
        let value = raw.and_then(Value::as_str).ok_or(Error::Denied)?;
        let parsed = match &field.kind {
            FormFieldKind::Text {} if value.chars().count() <= 1000 && value.len() <= 4000 => {
                value.into()
            }
            FormFieldKind::Number {} if value.len() <= 64 && value.trim() == value => {
                let number: serde_json::Number =
                    serde_json::from_str(value).map_err(|_| Error::Denied)?;
                Value::Number(number)
            }
            FormFieldKind::Date {} => date(value)?.into(),
            FormFieldKind::Choice { options } => {
                let choice = value.parse::<usize>().map_err(|_| Error::Denied)?;
                if choice.to_string() != value {
                    return Err(Error::Denied);
                }
                options.get(choice).ok_or(Error::Denied)?.id.clone().into()
            }
            _ => return Err(Error::Denied),
        };
        result.insert(field.id.clone(), parsed);
    }
    if !valid_values(fields, &result) {
        return Err(Error::Denied);
    }
    Ok(result)
}

pub(super) fn valid_values(fields: &[FormField], values: &Submitted) -> bool {
    if values.len() > fields.len()
        || serde_json::to_vec(values).map_or(true, |bytes| bytes.len() > MAX_SUBMITTED_BYTES)
        || values
            .keys()
            .any(|key| !fields.iter().any(|field| &field.id == key))
    {
        return false;
    }
    fields.iter().all(|field| match values.get(&field.id) {
        None => !field.required,
        Some(value) => match &field.kind {
            FormFieldKind::Text {} => value
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 4000 && s.chars().count() <= 1000),
            FormFieldKind::Number {} => value.is_number(),
            FormFieldKind::Date {} => value.as_str().is_some_and(|s| date(s).ok() == Some(s)),
            FormFieldKind::Choice { options } => value
                .as_str()
                .is_some_and(|s| options.iter().any(|option| option.id == s)),
        },
    })
}

fn date(value: &str) -> Result<&str, Error> {
    // The native date picker submits YYYY-MM-DD, optionally followed by its
    // documented numeric timezone. A calendar date is independent of that zone.
    if value.len() != 10 && value.len() != 16 {
        return Err(Error::Denied);
    }
    if value.len() == 16 {
        let suffix = &value.as_bytes()[10..];
        if suffix[0] != b' '
            || !matches!(suffix[1], b'+' | b'-')
            || !suffix[2..].iter().all(u8::is_ascii_digit)
            || value[12..14].parse::<u8>().map_or(true, |v| v > 23)
            || value[14..16].parse::<u8>().map_or(true, |v| v > 59)
        {
            return Err(Error::Denied);
        }
    }
    let day = value.get(..10).ok_or(Error::Denied)?;
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
        .map_err(|_| Error::Denied)?;
    let parsed = time::Date::parse(day, &format).map_err(|_| Error::Denied)?;
    if parsed.to_string() != day {
        return Err(Error::Denied);
    }
    Ok(day)
}

#[cfg(test)]
#[path = "lark_form_tests.rs"]
mod tests;
