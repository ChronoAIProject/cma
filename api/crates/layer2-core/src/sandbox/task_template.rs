//! Versioned recommended task inputs; compilation never executes inputs or fetches links.
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{code}")]
pub struct TaskTemplateError {
    pub code: &'static str,
    pub field: Option<&'static str>,
}

impl TaskTemplateError {
    pub fn new(code: &'static str, field: Option<&'static str>) -> Self {
        Self { code, field }
    }
}

pub fn compile_task_template(
    id: &str,
    version: u32,
    fields: &BTreeMap<String, String>,
) -> Result<String, TaskTemplateError> {
    let error = |code| TaskTemplateError::new(code, None);
    if ![1, 2].contains(&version) {
        return Err(error("task_template_version_unsupported"));
    }
    let required: &[(&str, &str)] = match id {
        "research" => &[("topic", "[topic]")],
        "spreadsheet" => &[("link", "[link]")],
        "document" => &[
            ("type", "[type of document]"),
            ("subject", "[subject]"),
            ("audience", "[audience]"),
        ],
        "website" => &[("project", "[project]")],
        "code" => &[("language", "[language]"), ("behavior", "[does what]")],
        _ => return Err(error("task_template_id_unknown")),
    };
    if fields
        .keys()
        .any(|key| !required.iter().any(|(name, _)| key == name))
    {
        return Err(error("task_template_field_unknown"));
    }
    for &(name, placeholder) in required {
        let value = fields
            .get(name)
            .map(|value| value.trim())
            .unwrap_or_default();
        if value.is_empty() {
            return Err(TaskTemplateError::new(
                "task_template_field_required",
                Some(name),
            ));
        }
        if value == placeholder {
            return Err(TaskTemplateError::new(
                "task_template_field_placeholder",
                Some(name),
            ));
        }
        if value.len() > super::MAX_FIRST_MESSAGE_BYTES {
            return Err(error("task_template_message_too_large"));
        }
    }
    let value = |name: &str| fields[name].trim();
    let message = match id {
        "research" if version == 2 => format!(
            "Research the following topic:\n\n{}\n\nSummarize the key findings, with links to sources.",
            value("topic")
        ),
        "code" if version == 2 => {
            let behavior = value("behavior");
            let punctuation = if behavior.ends_with(['.', '!', '?', '。', '！', '？']) { "" } else { "." };
            format!("Write a {} script for the following task:\n\n{behavior}{punctuation}", value("language"))
        }
        "research" => format!(
            "Research {} and summarize the key findings, with links to sources.",
            value("topic")
        ),
        "spreadsheet" => format!(
            "Analyze the spreadsheet at {} and point out trends, outliers and what to do next.",
            value("link")
        ),
        "document" => format!(
            "Write a {} about {} for {}.",
            value("type"),
            value("subject"),
            value("audience")
        ),
        "website" => format!(
            "Build a one-page website for {} and share a preview link.",
            value("project")
        ),
        "code" => format!(
            "Write a {} script that {}.",
            value("language"),
            value("behavior")
        ),
        _ => unreachable!("template id was validated"),
    };
    if message.len() > super::MAX_FIRST_MESSAGE_BYTES {
        return Err(error("task_template_message_too_large"));
    }
    Ok(message)
}

#[cfg(test)]
#[path = "task_template_tests.rs"]
mod tests;
