use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::{InputFieldDefinition, TriggerTask, TriggerValidationError, MAX_RENDERED_PROMPT_BYTES};

pub(super) fn validate(task: &TriggerTask) -> Result<(), TriggerValidationError> {
    let declared: BTreeSet<&str> = task
        .input_fields
        .iter()
        .map(|field| field.key.as_str())
        .collect();
    let referenced = tokens(&task.prompt_template)?;
    for key in &referenced {
        if !declared.contains(key.as_str()) {
            return Err(TriggerValidationError::field(
                "task.prompt_template",
                format!("references undeclared input field {key}"),
            ));
        }
    }
    for field in &task.input_fields {
        if !referenced.contains(&field.key) {
            return Err(TriggerValidationError::field(
                format!("task.input_fields.{}", field.key),
                "is declared but not referenced by the prompt template",
            ));
        }
    }
    Ok(())
}

pub(super) fn render(
    task: &TriggerTask,
    supplied: &BTreeMap<String, Value>,
) -> Result<String, TriggerValidationError> {
    validate(task)?;
    let definitions: BTreeMap<&str, &InputFieldDefinition> = task
        .input_fields
        .iter()
        .map(|field| (field.key.as_str(), field))
        .collect();
    for (key, value) in supplied {
        let field = definitions.get(key.as_str()).ok_or_else(|| {
            TriggerValidationError::field(
                "task.inputs",
                format!("{key} is not declared by this revision"),
            )
        })?;
        if !field.editable {
            return Err(TriggerValidationError::field(
                format!("task.inputs.{key}"),
                "is locked by the publisher revision",
            ));
        }
        field.validate_value(value)?;
    }

    let mut values = BTreeMap::new();
    for field in &task.input_fields {
        let value = supplied
            .get(&field.key)
            .or(field.default.as_ref())
            .ok_or_else(|| {
                TriggerValidationError::field(format!("task.inputs.{}", field.key), "is required")
            })?;
        field.validate_value(value)?;
        values.insert(field.key.as_str(), scalar(value)?);
    }

    let mut rendered = String::with_capacity(task.prompt_template.len());
    let mut rest = task.prompt_template.as_str();
    while let Some(open) = rest.find("{{") {
        rendered.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let close = after_open.find("}}").ok_or_else(|| {
            TriggerValidationError::field("task.prompt_template", "has an unmatched {{ token")
        })?;
        let key = &after_open[..close];
        let value = values.get(key).ok_or_else(|| {
            TriggerValidationError::field(
                "task.prompt_template",
                format!("references undeclared input field {key}"),
            )
        })?;
        rendered.push_str(value);
        rest = &after_open[close + 2..];
    }
    rendered.push_str(rest);
    if rendered.len() > MAX_RENDERED_PROMPT_BYTES {
        return Err(TriggerValidationError::field(
            "task.prompt_template",
            format!("renders beyond {MAX_RENDERED_PROMPT_BYTES} bytes"),
        ));
    }
    Ok(rendered)
}

fn tokens(template: &str) -> Result<BTreeSet<String>, TriggerValidationError> {
    let mut found = BTreeSet::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        if rest[..open].contains("}}") {
            return Err(TriggerValidationError::field(
                "task.prompt_template",
                "has an unmatched }} token",
            ));
        }
        let after_open = &rest[open + 2..];
        let close = after_open.find("}}").ok_or_else(|| {
            TriggerValidationError::field("task.prompt_template", "has an unmatched {{ token")
        })?;
        let key = &after_open[..close];
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(TriggerValidationError::field(
                "task.prompt_template",
                "contains an invalid input token",
            ));
        }
        found.insert(key.to_string());
        rest = &after_open[close + 2..];
    }
    if rest.contains("}}") {
        return Err(TriggerValidationError::field(
            "task.prompt_template",
            "has an unmatched }} token",
        ));
    }
    Ok(found)
}

fn scalar(value: &Value) -> Result<String, TriggerValidationError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(TriggerValidationError::new(
            "prompt fields may contain only text, integer, or boolean values",
        )),
    }
}
