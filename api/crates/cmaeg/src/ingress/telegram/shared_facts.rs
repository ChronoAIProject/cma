//! Native Telegram contact/location facts are display content, never authority.
use cmaeg_core::activity::forbidden_text_char;
use serde_json::Value;

fn text(value: &Value, limit: usize) -> Result<&str, ()> {
    value
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= limit && !s.chars().any(forbidden_text_char))
        .ok_or(())
}

fn quoted(value: &Value, limit: usize) -> Result<String, ()> {
    serde_json::to_string(text(value, limit)?).map_err(|_| ())
}

fn location(value: &Value) -> Result<String, ()> {
    let coordinate = |key: &str, limit: f64| {
        value[key]
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= limit)
            .ok_or(())
    };
    let latitude = coordinate("latitude", 90.0)?;
    let longitude = coordinate("longitude", 180.0)?;
    let mut result = format!("Latitude: {latitude}\nLongitude: {longitude}");
    if let Some(accuracy) = value.get("horizontal_accuracy") {
        let accuracy = accuracy
            .as_f64()
            .filter(|n| n.is_finite() && (0.0..=1500.0).contains(n))
            .ok_or(())?;
        result.push_str(&format!("\nAccuracy (metres): {accuracy}"));
    }
    if let Some(period) = value.get("live_period") {
        period.as_u64().filter(|n| *n > 0).ok_or(())?;
        result.push_str("\nLive location snapshot (not a continuous location feed)");
    }
    if let Some(heading) = value.get("heading") {
        let heading = heading
            .as_u64()
            .filter(|n| (1..=360).contains(n))
            .ok_or(())?;
        result.push_str(&format!("\nHeading (degrees): {heading}"));
    }
    if let Some(radius) = value.get("proximity_alert_radius") {
        let radius = radius.as_u64().filter(|n| *n > 0).ok_or(())?;
        result.push_str(&format!("\nProximity alert radius (metres): {radius}"));
    }
    Ok(result)
}

/// Uses only fields on the authenticated message. A shared contact's user_id,
/// vCard and venue-provider identifiers are not actor IDs, destinations or URLs.
pub(crate) fn extract(message: &Value) -> Result<Option<String>, ()> {
    let present: Vec<_> = ["contact", "location", "venue"]
        .into_iter()
        .filter(|key| message.get(*key).is_some())
        .collect();
    if present.is_empty() {
        return Ok(None);
    }
    // Venue messages can also include their identical location for compatibility.
    let venue_location =
        present == ["location", "venue"] && message["location"] == message["venue"]["location"];
    if (present.len() != 1 && !venue_location)
        || [
            "text",
            "caption",
            "photo",
            "document",
            "audio",
            "video",
            "voice",
            "sticker",
            "video_note",
            "animation",
            "poll",
            "dice",
            "game",
            "invoice",
            "successful_payment",
        ]
        .iter()
        .any(|key| message.get(*key).is_some())
    {
        return Err(());
    }
    let result = if let Some(contact) = message.get("contact") {
        let first_name = quoted(&contact["first_name"], 256)?;
        let phone = text(&contact["phone_number"], 80)?;
        if !phone.bytes().any(|b| b.is_ascii_digit())
            || !phone
                .bytes()
                .all(|b| b.is_ascii_digit() || b"+(). -".contains(&b))
        {
            return Err(());
        }
        let mut result = format!(
            "Shared contact:\nFirst name: {first_name}\nPhone: {}",
            serde_json::to_string(phone).map_err(|_| ())?
        );
        if let Some(last_name) = contact.get("last_name").filter(|v| v.as_str() != Some("")) {
            result.push_str(&format!("\nLast name: {}", quoted(last_name, 256)?));
        }
        result
    } else if let Some(venue) = message.get("venue") {
        if venue["location"].get("live_period").is_some() {
            return Err(());
        }
        format!(
            "Shared venue:\nTitle: {}\nAddress: {}\n{}",
            quoted(&venue["title"], 256)?,
            quoted(&venue["address"], 1024)?,
            location(&venue["location"])?
        )
    } else {
        format!("Shared location:\n{}", location(&message["location"])?)
    };
    Ok(Some(result))
}

#[cfg(test)]
#[path = "shared_facts_tests.rs"]
mod tests;
