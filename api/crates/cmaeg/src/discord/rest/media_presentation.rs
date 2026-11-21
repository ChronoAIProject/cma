//! Generated previews are observed presentation, never writable rich-embed payloads.
use super::RestError;
use crate::native_media::model::Presentation;
use serde_json::{Map, Value};
const SPOILER: u64 = 1 << 3;
const ANIMATED: u64 = 1 << 5;

pub(super) fn observe(
    message: &Value,
    attachment: &Value,
) -> Result<(Presentation, bool), RestError> {
    let flags = bits(message, "flags")?;
    let attachment_flags = bits(attachment, "flags")?;
    if flags & !((1 << 2) | (1 << 12)) != 0 || attachment_flags & !(SPOILER | ANIMATED) != 0 {
        return Err(RestError::Unknown);
    }
    if attachment["filename"]
        .as_str()
        .is_some_and(|name| name.starts_with("SPOILER_"))
        && attachment_flags & SPOILER == 0
    {
        return Err(RestError::Unknown);
    }
    let mut embeds = match message.get("embeds") {
        None => vec![],
        Some(value) => value.as_array().cloned().ok_or(RestError::Unknown)?,
    };
    if embeds.len() > 128 {
        return Err(RestError::Unknown);
    }
    let mut eligible = embeds.len() <= 10;
    for embed in &mut embeds {
        eligible &= canonical_embed(embed)?;
    }
    eligible &= embeds.iter().map(embed_characters).sum::<usize>() <= 6000;
    let presentation = Presentation::Discord {
        message_type: message["type"].as_u64().ok_or(RestError::Unknown)?,
        reply: reply(message)?,
        embeds,
        flags,
        attachment_flags,
    };
    eligible &= serde_json::to_vec(&presentation)
        .map_err(|_| RestError::Unknown)?
        .len()
        <= 32 * 1024;
    Ok((presentation, eligible))
}
fn reply(message: &Value) -> Result<Option<Value>, RestError> {
    let Some(value) = message.get("message_reference").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let map = value.as_object().ok_or(RestError::Unknown)?;
    for (key, value) in map {
        let valid = match key.as_str() {
            "type" => value.as_u64() == Some(0),
            "message_id" | "channel_id" | "guild_id" => value.as_str().is_some_and(super::valid_id),
            "fail_if_not_exists" => value.is_boolean(),
            _ => false,
        };
        if !valid {
            return Err(RestError::Unknown);
        }
    }
    Ok(Some(value.clone()))
}
pub(super) fn spoiler(presentation: Option<&Presentation>) -> Result<bool, RestError> {
    match presentation {
        Some(Presentation::Discord {
            attachment_flags, ..
        }) => Ok(attachment_flags & SPOILER != 0),
        None => Ok(false),
        _ => Err(RestError::Rejected),
    }
}
fn bits(value: &Value, key: &str) -> Result<u64, RestError> {
    value
        .get(key)
        .map_or(Ok(0), |v| v.as_u64().ok_or(RestError::Unknown))
}
fn canonical_embed(value: &mut Value) -> Result<bool, RestError> {
    let map = value.as_object_mut().ok_or(RestError::Unknown)?;
    if !map
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| matches!(kind, "link" | "article" | "image" | "video" | "gifv"))
        || map.contains_key("components")
    {
        return Err(RestError::Unknown);
    }
    let allowed = [
        "type",
        "title",
        "description",
        "url",
        "timestamp",
        "color",
        "footer",
        "image",
        "thumbnail",
        "video",
        "provider",
        "author",
        "fields",
        "flags",
    ];
    let mut eligible = map.keys().all(|key| allowed.contains(&key.as_str()));
    for (key, limit) in [
        ("title", 256),
        ("description", 4096),
        ("url", 8192),
        ("timestamp", 64),
    ] {
        eligible &= text(map, key, limit)?;
    }
    if map
        .get("color")
        .is_some_and(|v| v.as_u64().is_none_or(|n| n > 0xffffff))
    {
        return Err(RestError::Unknown);
    }
    if map.get("flags").is_some_and(|v| v.as_u64() != Some(0)) {
        eligible = false;
    }
    for key in ["image", "thumbnail", "video"] {
        if let Some(value) = map.get_mut(key) {
            eligible &= media(value)?;
        }
    }
    for (key, keys) in [
        ("provider", &["name", "url"][..]),
        ("author", &["name", "url", "icon_url", "proxy_icon_url"][..]),
        ("footer", &["text", "icon_url", "proxy_icon_url"][..]),
    ] {
        if let Some(value) = map.get_mut(key) {
            let fields = value.as_object_mut().ok_or(RestError::Unknown)?;
            eligible &= fields.keys().all(|key| keys.contains(&key.as_str()));
            fields.remove("proxy_icon_url");
            for (field, value) in fields.iter_mut() {
                let limit = if field == "text" {
                    2048
                } else if field == "name" {
                    256
                } else {
                    8192
                };
                eligible &= string(value, limit)?;
                if field.ends_with("url") {
                    canonical_url(value)?;
                }
            }
        }
    }
    if let Some(value) = map.get_mut("fields") {
        let fields = value.as_array_mut().ok_or(RestError::Unknown)?;
        if fields.len() > 128 {
            return Err(RestError::Unknown);
        }
        eligible &= fields.len() <= 25;
        for field in fields {
            let map = field.as_object_mut().ok_or(RestError::Unknown)?;
            eligible &= map
                .keys()
                .all(|key| ["name", "value", "inline"].contains(&key.as_str()));
            eligible &= text(map, "name", 256)?;
            eligible &= text(map, "value", 1024)?;
            if !map.contains_key("name")
                || !map.contains_key("value")
                || map.get("inline").is_some_and(|v| !v.is_boolean())
            {
                return Err(RestError::Unknown);
            }
        }
    }
    // Only documented signed attachment URL parameters are non-semantic.
    if let Some(value) = map.get_mut("url") {
        canonical_url(value)?;
    }
    bounded(value, 0)?;
    Ok(eligible)
}
fn media(value: &mut Value) -> Result<bool, RestError> {
    let map = value.as_object_mut().ok_or(RestError::Unknown)?;
    let keys = [
        "url",
        "proxy_url",
        "height",
        "width",
        "content_type",
        "placeholder",
        "placeholder_version",
        "description",
        "flags",
    ];
    let mut eligible = map.keys().all(|key| keys.contains(&key.as_str()));
    map.remove("proxy_url");
    for key in ["url", "content_type", "placeholder", "description"] {
        eligible &= text(map, key, if key == "url" { 8192 } else { 4096 })?;
    }
    for key in ["height", "width", "placeholder_version"] {
        if map.get(key).is_some_and(|v| v.as_u64().is_none()) {
            return Err(RestError::Unknown);
        }
    }
    if map
        .get("flags")
        .is_some_and(|v| v.as_u64().is_none_or(|n| n & !ANIMATED != 0))
    {
        eligible = false;
    }
    if let Some(value) = map.get_mut("url") {
        canonical_url(value)?;
    }
    Ok(eligible)
}
fn text(map: &Map<String, Value>, key: &str, limit: usize) -> Result<bool, RestError> {
    map.get(key).map_or(Ok(true), |v| string(v, limit))
}
fn string(value: &Value, limit: usize) -> Result<bool, RestError> {
    let text = value.as_str().ok_or(RestError::Unknown)?;
    if text.len() > 32768 || text.contains('\0') {
        return Err(RestError::Unknown);
    }
    Ok(text.chars().count() <= limit)
}
fn canonical_url(value: &mut Value) -> Result<(), RestError> {
    let text = value.as_str().ok_or(RestError::Unknown)?;
    let Ok(mut url) = reqwest::Url::parse(text) else {
        return Ok(());
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
        || !url.path().starts_with("/attachments/")
    {
        return Ok(());
    }
    let keep: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| !matches!(key.as_ref(), "ex" | "is" | "hm"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    if !keep.is_empty() {
        url.query_pairs_mut().extend_pairs(keep);
    }
    *value = url.to_string().into();
    Ok(())
}
fn bounded(value: &Value, depth: usize) -> Result<(), RestError> {
    if depth > 6 {
        return Err(RestError::Unknown);
    }
    match value {
        Value::Object(map) => {
            if map.len() > 32 {
                return Err(RestError::Unknown);
            }
            for (key, child) in map {
                if key.len() > 128 {
                    return Err(RestError::Unknown);
                }
                bounded(child, depth + 1)?;
            }
        }
        Value::Array(values) => {
            if values.len() > 128 {
                return Err(RestError::Unknown);
            }
            for child in values {
                bounded(child, depth + 1)?;
            }
        }
        Value::String(text) if text.len() > 32768 => return Err(RestError::Unknown),
        _ => {}
    }
    Ok(())
}
pub(super) fn asset_metadata(value: &Value) -> Result<Option<Value>, RestError> {
    let mut facts = Map::new();
    for key in [
        "title",
        "placeholder",
        "placeholder_version",
        "duration_secs",
    ] {
        if let Some(value) = value.get(key).filter(|v| !v.is_null()) {
            match key {
                "title" | "placeholder" if !string(value, 4096)? => return Err(RestError::Unknown),
                "title" | "placeholder" => {}
                "placeholder_version" if value.as_u64().is_none() => {
                    return Err(RestError::Unknown)
                }
                _ if value.as_f64().is_none_or(|v| !v.is_finite() || v < 0.0) => {
                    return Err(RestError::Unknown)
                }
                _ => {}
            }
            facts.insert(key.into(), value.clone());
        }
    }
    Ok((!facts.is_empty()).then_some(Value::Object(facts)))
}

fn embed_characters(value: &Value) -> usize {
    let length = |v: &Value| v.as_str().map_or(0, |s| s.chars().count());
    let mut total = length(&value["title"])
        + length(&value["description"])
        + length(&value["footer"]["text"])
        + length(&value["author"]["name"]);
    if let Some(fields) = value["fields"].as_array() {
        for field in fields {
            total += length(&field["name"]) + length(&field["value"]);
        }
    }
    total
}
