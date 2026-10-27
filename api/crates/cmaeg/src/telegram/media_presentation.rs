//! Retained plain-caption entities and message-level media presentation.
use super::Error;
use crate::native_media::model::{CaptionEntity, Presentation};
use cmaeg_core::message::NativeMediaKind;
use serde_json::{json, Map, Value};

pub(super) fn observe(
    value: &Value,
    caption: &str,
    kind: NativeMediaKind,
) -> Result<(Presentation, bool), Error> {
    let mut entities: Vec<CaptionEntity> = match value.get("caption_entities") {
        None => vec![],
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| Error::Unconfirmed)?,
    };
    if entities.len() > 1024 {
        return Err(Error::Unconfirmed);
    }
    let mut boundaries = vec![0u32];
    for c in caption.chars() {
        boundaries.push(boundaries.last().unwrap() + c.len_utf16() as u32);
    }
    entities.sort_by(|a, b| (a.offset, a.length, &a.r#type).cmp(&(b.offset, b.length, &b.r#type)));
    let mut previous_end = 0;
    for entity in &entities {
        let end = entity
            .offset
            .checked_add(entity.length)
            .ok_or(Error::Unconfirmed)?;
        if !matches!(
            entity.r#type.as_str(),
            "mention" | "hashtag" | "cashtag" | "bot_command" | "url" | "email" | "phone_number"
        ) || entity.length == 0
            || entity.offset < previous_end
            || boundaries.binary_search(&entity.offset).is_err()
            || boundaries.binary_search(&end).is_err()
        {
            return Err(Error::Unconfirmed);
        }
        previous_end = end;
    }
    let caption_above = boolean(value, "show_caption_above_media")?;
    let spoiler = boolean(value, "has_media_spoiler")?;
    if (caption_above || spoiler)
        && !matches!(kind, NativeMediaKind::Image | NativeMediaKind::Video)
    {
        return Err(Error::Unconfirmed);
    }
    let eligible = entities.len() <= 128;
    let presentation = Presentation::Telegram {
        entities,
        caption_above,
        spoiler,
    };
    let bounded = serde_json::to_vec(&presentation)
        .map_err(|_| Error::Unconfirmed)?
        .len()
        <= 16 * 1024;
    Ok((presentation, eligible && bounded))
}
fn boolean(value: &Value, field: &str) -> Result<bool, Error> {
    value
        .get(field)
        .map_or(Ok(false), |v| v.as_bool().ok_or(Error::Unconfirmed))
}
pub(super) fn apply(
    payload: &mut Value,
    presentation: Option<&Presentation>,
    replace_caption: bool,
    replacement: bool,
    kind: NativeMediaKind,
) -> Result<(), Error> {
    let Some(Presentation::Telegram {
        entities,
        caption_above,
        spoiler,
    }) = presentation
    else {
        return if presentation.is_none() {
            payload["caption_entities"] = json!([]);
            Ok(())
        } else {
            Err(Error::Rejected)
        };
    };
    payload["caption_entities"] = if replace_caption {
        json!([])
    } else {
        json!(entities)
    };
    if matches!(kind, NativeMediaKind::Image | NativeMediaKind::Video) {
        payload["show_caption_above_media"] = (*caption_above).into();
        if replacement {
            payload["has_spoiler"] = (*spoiler).into();
        }
    }
    Ok(())
}
pub(super) fn asset_metadata(value: &Value) -> Result<Option<Value>, Error> {
    let mut facts = Map::new();
    for key in [
        "duration",
        "performer",
        "title",
        "thumbnail",
        "cover",
        "start_timestamp",
    ] {
        if let Some(value) = value.get(key) {
            let mut value = value.clone();
            stable_asset(&mut value, 0)?;
            facts.insert(key.into(), value);
        }
    }
    Ok((!facts.is_empty()).then_some(Value::Object(facts)))
}
fn stable_asset(value: &mut Value, depth: usize) -> Result<(), Error> {
    if depth > 3 {
        return Err(Error::Unconfirmed);
    }
    match value {
        Value::Object(map) => {
            if map.len() > 12 {
                return Err(Error::Unconfirmed);
            }
            map.remove("file_id");
            for child in map.values_mut() {
                stable_asset(child, depth + 1)?;
            }
        }
        Value::Array(items) => {
            if items.len() > 24 {
                return Err(Error::Unconfirmed);
            }
            for child in items {
                stable_asset(child, depth + 1)?;
            }
        }
        Value::String(text) if text.len() > 1024 || text.chars().any(char::is_control) => {
            return Err(Error::Unconfirmed)
        }
        _ => {}
    }
    Ok(())
}
