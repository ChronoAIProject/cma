//! Plain, single-pass substitution. Event data can never become template syntax.
use cmaeg_core::{cloudevent::CloudEvent, profile::canonical_json};
const MARKER: &str = "\n[truncated]";
pub fn render(template: &str, event: &CloudEvent) -> String {
    let data = bounded(
        &event
            .data
            .as_ref()
            .map(|value| canonical_json(value.clone()))
            .unwrap_or_else(|| "null".into()),
        8192,
    );
    let mut output = String::new();
    let mut remaining = template;
    while let Some(open) = remaining.find("{{") {
        output.push_str(&remaining[..open]);
        let Some(close) = remaining[open + 2..].find("}}") else {
            remaining = &remaining[open..];
            break;
        };
        let end = open + 2 + close + 2;
        let name = &remaining[open + 2..end - 2];
        let replacement = match name {
            "type" => event.r#type.clone(),
            "source" => event.source.clone(),
            "subject" => event.subject.clone().unwrap_or_default(),
            "id" => event.id.clone(),
            "time" => event.time.clone().unwrap_or_default(),
            "data" => data.clone(),
            path if path.starts_with("data.") => event
                .data
                .as_ref()
                .and_then(|value| {
                    path.split('.')
                        .skip(1)
                        .try_fold(value, |value, key| value.get(key))
                })
                .map(|value| match value {
                    serde_json::Value::String(value) => value.clone(),
                    value => canonical_json(value.clone()),
                })
                .unwrap_or_default(),
            _ => remaining[open..end].to_owned(),
        };
        output.push_str(&replacement);
        remaining = &remaining[end..];
        if output.len() >= 32768 {
            return bounded(&output, 32768);
        }
    }
    output.push_str(remaining);
    bounded(&output, 32768)
}
fn bounded(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit - MARKER.len();
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{MARKER}", &value[..end])
}
