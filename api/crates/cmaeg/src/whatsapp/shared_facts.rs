//! Bounded display facts from signed private contact/location messages.
//! Contact identities and provider URLs never become locators or authority.
use super::webhook::{Content, Error};
use cmaeg_core::activity::forbidden_text_char;
use serde_json::Value;

const MAX_CONTACTS: usize = 10;
const MAX_PHONES: usize = 5;
const MAX_NAME_BYTES: usize = 256;
const MAX_PHONE_BYTES: usize = 80;
const MAX_ADDRESS_BYTES: usize = 1024;
const MAX_TEXT_BYTES: usize = 8192;

pub(super) fn extract(raw: &Value, kind: &str) -> Result<Content, Error> {
    let text = match kind {
        "contacts" => contacts(&raw["contacts"]),
        "location" => location(&raw["location"]),
        _ => Err(Error::Message),
    }?;
    if text.len() > MAX_TEXT_BYTES {
        return Err(Error::Message);
    }
    Ok(Content::Text { text })
}

fn text(value: &Value, cap: usize) -> Result<&str, Error> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= cap && !s.chars().any(forbidden_text_char))
        .ok_or(Error::Message)
}

fn quoted(value: &str) -> String {
    // Names cannot impersonate our field labels using quotes or delimiters.
    serde_json::to_string(value).expect("display text serializes")
}

fn contacts(value: &Value) -> Result<String, Error> {
    let contacts = value
        .as_array()
        .filter(|items| !items.is_empty() && items.len() <= MAX_CONTACTS)
        .ok_or(Error::Message)?;
    let mut result = String::from("Shared contacts:");
    for (index, contact) in contacts.iter().enumerate() {
        let name = text(&contact["name"]["formatted_name"], MAX_NAME_BYTES)?;
        result.push_str(&format!("\n{}. Name: {}", index + 1, quoted(name)));
        if let Some(value) = contact.get("phones") {
            let phones = value
                .as_array()
                .filter(|items| items.len() <= MAX_PHONES)
                .ok_or(Error::Message)?;
            for phone in phones {
                let phone = text(&phone["phone"], MAX_PHONE_BYTES)?;
                // Preserve a bounded display number, not a Meta user identifier.
                // Never substitute wa_id or infer a recipient from this field.
                if !phone.bytes().any(|b| b.is_ascii_digit())
                    || !phone
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b"+(). -".contains(&b))
                {
                    return Err(Error::Message);
                }
                result.push_str(&format!("; Phone: {}", quoted(phone)));
            }
        }
    }
    Ok(result)
}

fn coordinate(value: &Value, limit: f64) -> Result<f64, Error> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && (-limit..=limit).contains(n))
        .ok_or(Error::Message)
}

fn location(value: &Value) -> Result<String, Error> {
    let latitude = coordinate(&value["latitude"], 90.0)?;
    let longitude = coordinate(&value["longitude"], 180.0)?;
    let mut result = format!("Shared location:\nLatitude: {latitude}\nLongitude: {longitude}");
    for (field, label, cap) in [
        ("name", "Name", MAX_NAME_BYTES),
        ("address", "Address", MAX_ADDRESS_BYTES),
    ] {
        if let Some(value) = value.get(field) {
            result.push_str(&format!("\n{label}: {}", quoted(text(value, cap)?)));
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "shared_facts_tests.rs"]
pub(crate) mod tests;
