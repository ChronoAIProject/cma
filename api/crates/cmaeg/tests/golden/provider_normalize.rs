//! Normalize captured provider calls into their golden form.
//!
//! The gateway has no injectable clock, nonce or id source on these paths, so
//! the values a run cannot reproduce are replaced after capture, and only
//! these, each by name:
//!
//! - in JSON bodies, the string values of the keys in [`NAMED_FIELDS`]: sealed
//!   channel ids and event references, derived conversation ids, the channel
//!   hash in the metadata and activity timestamps;
//! - in rendered input text, the attachment-fence nonce of `<media-…>` markers;
//! - in paths, the binding and conversation segments;
//! - the random suffix of a test turn's `test_` idempotency key.
//!
//! Each distinct value becomes `<kind:N>`, numbered by first appearance within
//! one scenario, so a value repeated across calls (a conversation in a path
//! and in the following body) stays visibly the same value. Bodies are
//! rewritten in their raw bytes, `"key":"value"` pair by pair, so key order,
//! spacing and every other byte the gateway sent remain pinned.
//!
//! Credentials are never stored: the `authorization` header and a body's
//! `agent_key` are recorded as the SHA-256 of their exact value.
use super::fake_provider::ReceivedRequest;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Body keys whose string values differ on every run, with their placeholder kind.
const NAMED_FIELDS: &[(&str, &str)] = &[
    ("channel_id", "channel_id"),
    ("conversation", "conversation_id"),
    ("conversation_id", "conversation_id"),
    ("event_ref", "event_ref"),
    ("cmaeg.event_ref", "event_ref"),
    ("cmaeg.channel", "channel_hash"),
    ("occurred_at", "timestamp"),
];
/// Body keys whose string values are credentials, stored only as a hash.
const CREDENTIAL_FIELDS: &[&str] = &["agent_key"];
/// Path segments followed by a per-run identifier, with its placeholder kind.
const NAMED_SEGMENTS: &[(&str, &str)] = &[
    ("bindings", "binding_id"),
    ("conversations", "conversation_id"),
];
const NONCE_BYTES: usize = 12;

/// One provider call in its golden form.
#[derive(Debug, Serialize)]
pub struct CapturedCall {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub idempotency_key: Option<String>,
    pub authorization: Option<String>,
    pub body: String,
}

/// Placeholder assignment for one scenario.
#[derive(Default)]
pub struct Normalizer {
    assigned: BTreeMap<(&'static str, String), String>,
    counts: BTreeMap<&'static str, usize>,
}

impl Normalizer {
    /// Normalize every call of one scenario, in arrival order, into the
    /// pretty JSON a fixture holds.
    pub fn fixture(calls: &[ReceivedRequest]) -> String {
        let mut normalizer = Self::default();
        let calls: Vec<_> = calls.iter().map(|call| normalizer.call(call)).collect();
        let mut text = serde_json::to_string_pretty(&calls).expect("captures serialize");
        text.push('\n');
        text
    }

    pub fn call(&mut self, call: &ReceivedRequest) -> CapturedCall {
        CapturedCall {
            method: call.method.clone(),
            path: self.path(&call.path),
            content_type: call.content_type.clone(),
            idempotency_key: call.idempotency_key.as_deref().map(|key| self.key(key)),
            authorization: call.authorization.as_deref().map(sha256),
            body: self.body(&call.body),
        }
    }

    fn placeholder(&mut self, kind: &'static str, raw: &str) -> String {
        if let Some(existing) = self.assigned.get(&(kind, raw.to_owned())) {
            return existing.clone();
        }
        let count = self.counts.entry(kind).or_default();
        *count += 1;
        let placeholder = format!("<{kind}:{count}>");
        self.assigned
            .insert((kind, raw.to_owned()), placeholder.clone());
        placeholder
    }

    fn path(&mut self, path: &str) -> String {
        let mut previous = "";
        let mut segments = Vec::new();
        for segment in path.split('/') {
            let kind = NAMED_SEGMENTS
                .iter()
                .find(|(name, _)| *name == previous)
                .map(|(_, kind)| *kind);
            segments.push(match kind {
                Some(kind) => self.placeholder(kind, segment),
                None => segment.to_owned(),
            });
            previous = segment;
        }
        segments.join("/")
    }

    fn key(&mut self, key: &str) -> String {
        match key.strip_prefix("test_") {
            Some(suffix) if suffix.len() == 32 && is_lower_hex(suffix) => {
                format!("test_{}", self.placeholder("test_turn", suffix))
            }
            _ => key.to_owned(),
        }
    }

    fn body(&mut self, raw: &str) -> String {
        if raw.is_empty() {
            return String::new();
        }
        let value: Value = serde_json::from_str(raw).expect("provider bodies are JSON");
        let mut pairs = Vec::new();
        named_pairs(&value, &mut pairs);
        let mut body = raw.to_owned();
        for (key, raw_value) in pairs {
            let replacement = if CREDENTIAL_FIELDS.contains(&key.as_str()) {
                sha256(&raw_value)
            } else if let Some((_, kind)) = NAMED_FIELDS.iter().find(|(name, _)| *name == key) {
                self.placeholder(kind, &raw_value)
            } else {
                continue;
            };
            let key = json_string(&key);
            body = body.replace(
                &format!("{key}:{}", json_string(&raw_value)),
                &format!("{key}:{}", json_string(&replacement)),
            );
        }
        self.fence_nonces(&body)
    }

    /// Replace each `media-<nonce>` marker of an attachment fence.
    fn fence_nonces(&mut self, body: &str) -> String {
        let mut output = String::with_capacity(body.len());
        let mut rest = body;
        while let Some(start) = rest.find("media-") {
            let (before, marker) = rest.split_at(start + "media-".len());
            output.push_str(before);
            let candidate = marker.get(..NONCE_BYTES).unwrap_or_default();
            if candidate.len() == NONCE_BYTES && is_lower_hex(candidate) {
                output.push_str(&self.placeholder("fence_nonce", candidate));
                rest = &marker[NONCE_BYTES..];
            } else {
                rest = marker;
            }
        }
        output.push_str(rest);
        output
    }
}

/// Every `(key, string value)` pair whose key is named, in document order.
fn named_pairs(value: &Value, pairs: &mut Vec<(String, String)>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let named = CREDENTIAL_FIELDS.contains(&key.as_str())
                    || NAMED_FIELDS.iter().any(|(name, _)| name == key);
                match value {
                    Value::String(text) if named => pairs.push((key.clone(), text.clone())),
                    _ => named_pairs(value, pairs),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| named_pairs(item, pairs)),
        _ => {}
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}
