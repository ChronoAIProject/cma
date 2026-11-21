use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

pub(super) struct Symbols {
    current: BTreeMap<String, Value>,
    captured: Vec<(Value, Value)>,
    pending_seeds: BTreeSet<String>,
}

impl Default for Symbols {
    fn default() -> Self {
        Self {
            current: BTreeMap::from([
                (
                    "ephemeral_credential".into(),
                    Value::String(uuid::Uuid::new_v4().simple().to_string()),
                ),
                (
                    "recent_admission".into(),
                    Value::from(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            - 1,
                    ),
                ),
                (
                    "interaction_expiry".into(),
                    Value::from(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            + 3600,
                    ),
                ),
            ]),
            captured: Vec::new(),
            pending_seeds: BTreeSet::new(),
        }
    }
}
impl Symbols {
    pub fn seed(&mut self, name: &str, actual: &Value, canonical: &Value) {
        self.capture(name, actual, canonical);
        self.pending_seeds.insert(name.to_owned());
    }

    pub fn assert_complete(&self) {
        assert!(
            self.pending_seeds.is_empty(),
            "unused seeded captures: {:?}",
            self.pending_seeds
        );
    }

    pub fn capture(&mut self, name: &str, actual: &Value, canonical: &Value) {
        self.pending_seeds.remove(name);
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "invalid capture symbol {name}"
        );
        assert!(
            actual.is_string() || actual.is_number(),
            "captures must identify scalar fields, never whole bodies"
        );
        assert_eq!(
            actual.is_string(),
            canonical.is_string(),
            "capture type differs for {name}"
        );
        assert_eq!(
            actual.is_number(),
            canonical.is_number(),
            "capture type differs for {name}"
        );
        if let Some((_, previous)) = self.captured.iter().find(|(value, _)| value == actual) {
            assert_eq!(
                previous, canonical,
                "one actual value has conflicting canonical captures"
            );
        }
        self.current.insert(name.to_owned(), actual.clone());
        self.captured.push((actual.clone(), canonical.clone()));
    }

    pub fn resolve(&self, value: &Value) -> Value {
        match value {
            Value::String(text)
                if text.starts_with('$')
                    && text[1..]
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') =>
            {
                self.current
                    .get(&text[1..])
                    .unwrap_or_else(|| panic!("undefined capture token {text}"))
                    .clone()
            }
            Value::String(text) => Value::String(self.resolve_text(text)),
            Value::Array(values) => {
                Value::Array(values.iter().map(|value| self.resolve(value)).collect())
            }
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), self.resolve(value)))
                    .collect(),
            ),
            _ => value.clone(),
        }
    }

    pub fn resolve_text(&self, text: &str) -> String {
        let mut result = String::new();
        let mut remaining = text;
        while let Some(start) = remaining.find('$') {
            result.push_str(&remaining[..start]);
            remaining = &remaining[start + 1..];
            let length = remaining
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            assert!(length > 0, "empty capture token");
            let name = &remaining[..length];
            let value = self
                .current
                .get(name)
                .unwrap_or_else(|| panic!("undefined capture token ${name}"));
            result.push_str(
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("non-string token ${name} embedded in text")),
            );
            remaining = &remaining[length..];
        }
        result.push_str(remaining);
        result
    }

    pub fn normalize(&self, value: Value) -> Value {
        if let Some((_, canonical)) = self.captured.iter().find(|(actual, _)| actual == &value) {
            return canonical.clone();
        }
        match value {
            Value::Array(values) => Value::Array(
                values
                    .into_iter()
                    .map(|value| self.normalize(value))
                    .collect(),
            ),
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, self.normalize(value)))
                    .collect(),
            ),
            _ => value,
        }
    }

    pub fn response_tokens(&self, actual: Value, expected: &Value) -> Value {
        match (actual, expected) {
            (actual, Value::String(template)) if template.contains('$') => {
                assert_eq!(actual, self.resolve(expected), "response template mismatch");
                expected.clone()
            }
            (Value::Array(values), Value::Array(templates)) => Value::Array(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| {
                        templates.get(index).map_or_else(
                            || value.clone(),
                            |template| self.response_tokens(value.clone(), template),
                        )
                    })
                    .collect(),
            ),
            (Value::Object(values), Value::Object(templates)) => Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| {
                        let value = templates.get(&key).map_or_else(
                            || value.clone(),
                            |template| self.response_tokens(value.clone(), template),
                        );
                        (key, value)
                    })
                    .collect(),
            ),
            (actual, _) => actual,
        }
    }
}

#[test]
fn named_captures_resolve_recursively_and_normalize_only_exact_scalars() {
    let mut symbols = Symbols::default();
    symbols.capture(
        "agent_id",
        &Value::String("agt_real".into()),
        &Value::String("agt_contract".into()),
    );
    assert_eq!(
        symbols.resolve(&serde_json::json!({"path":"/agents/$agent_id", "ids":["$agent_id"]})),
        serde_json::json!({"path":"/agents/agt_real", "ids":["agt_real"]})
    );
    assert_eq!(
        symbols.normalize(serde_json::json!({"id":"agt_real", "text":"prefix agt_real"})),
        serde_json::json!({"id":"agt_contract", "text":"prefix agt_real"})
    );
}

#[test]
#[should_panic(expected = "undefined capture token")]
fn undefined_capture_tokens_are_refused() {
    Symbols::default().resolve_text("/$missing");
}

#[test]
#[should_panic(expected = "captures must identify scalar fields")]
fn captures_cannot_hide_an_entire_response_body() {
    Symbols::default().capture(
        "all",
        &serde_json::json!({"field":1}),
        &serde_json::json!({"field":2}),
    );
}

#[test]
fn response_tokens_preserve_exact_surrounding_snippet_text() {
    let mut symbols = Symbols::default();
    symbols.capture(
        "id",
        &Value::String("public_real".into()),
        &Value::String("public_contract".into()),
    );
    let expected = serde_json::json!({"html":"<a href=\"/t/$id\">Launch</a>"});
    let actual = serde_json::json!({"html":"<a href=\"/t/public_real\">Launch</a>"});
    assert_eq!(symbols.response_tokens(actual, &expected), expected);
}

#[test]
#[should_panic(expected = "response template mismatch")]
fn response_tokens_do_not_mask_snippet_regressions() {
    let mut symbols = Symbols::default();
    symbols.capture(
        "id",
        &Value::String("public_real".into()),
        &Value::String("public_contract".into()),
    );
    symbols.response_tokens(
        serde_json::json!("/wrong/public_real"),
        &serde_json::json!("/t/$id"),
    );
}

#[test]
#[should_panic(expected = "undefined capture token")]
fn response_tokens_refuse_unbound_symbols() {
    Symbols::default().response_tokens(
        serde_json::json!("/t/unknown"),
        &serde_json::json!("/t/$unknown"),
    );
}
