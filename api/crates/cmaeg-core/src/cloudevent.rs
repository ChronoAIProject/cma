//! CloudEvents 1.0 structured and binary representations.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::{require_string, ValidationError};

pub const CLOUD_EVENT_SPEC_VERSION: &str = "1.0";
pub const MAX_CLOUD_EVENT_BYTES: usize = 256 * 1_024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CloudEvent {
    pub specversion: String,
    pub id: String,
    pub source: String,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacontenttype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataschema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeError {
    pub pointer: String,
    pub message: String,
}

impl CeError {
    fn new(pointer: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            pointer: pointer.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for CeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.pointer, self.message)
    }
}

impl std::error::Error for CeError {}

impl From<serde_json::Error> for CeError {
    fn from(error: serde_json::Error) -> Self {
        Self::new("/", format!("invalid JSON: {error}"))
    }
}

impl CloudEvent {
    /// Validate CloudEvents required attributes and the bounded JSON encoding.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.specversion != CLOUD_EVENT_SPEC_VERSION {
            return Err(ValidationError::new("/specversion", "must be 1.0"));
        }
        require_string(&self.id, "/id", 1, 512)?;
        require_string(&self.source, "/source", 1, 4_096)?;
        require_string(&self.r#type, "/type", 1, 512)?;
        if let Some(time) = &self.time {
            require_string(time, "/time", 1, 128)?;
        }
        if let Some(subject) = &self.subject {
            require_string(subject, "/subject", 1, 4_096)?;
        }
        for name in self.extensions.keys() {
            if name.len() > 20
                || name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                || [
                    "specversion",
                    "id",
                    "source",
                    "type",
                    "time",
                    "subject",
                    "datacontenttype",
                    "dataschema",
                    "data",
                ]
                .contains(&name.as_str())
            {
                return Err(ValidationError::new(
                    format!("/{name}"),
                    "extension name must be lowercase alphanumeric and non-reserved",
                ));
            }
        }
        let size = serde_json::to_vec(self)
            .map_err(|error| ValidationError::new("/", format!("cannot encode: {error}")))?
            .len();
        if size > MAX_CLOUD_EVENT_BYTES {
            return Err(ValidationError::new(
                "/",
                "serialized event exceeds 256 KiB",
            ));
        }
        Ok(())
    }
}

/// Parse a structured CloudEvents JSON body.
pub fn parse_structured(bytes: &[u8]) -> Result<CloudEvent, CeError> {
    if bytes.len() > MAX_CLOUD_EVENT_BYTES {
        return Err(CeError::new("/", "payload exceeds 256 KiB"));
    }
    let event: CloudEvent = serde_json::from_slice(bytes)?;
    event
        .validate()
        .map_err(|error| CeError::new(error.pointer, error.message))?;
    Ok(event)
}

/// Parse a binary-mode CloudEvents request from `ce-*` headers and a JSON body.
pub fn parse_binary(headers: &[(String, String)], body: &[u8]) -> Result<CloudEvent, CeError> {
    if body.len() > MAX_CLOUD_EVENT_BYTES {
        return Err(CeError::new("/data", "payload exceeds 256 KiB"));
    }
    let mut values = BTreeMap::new();
    for (name, value) in headers {
        let key = name.to_ascii_lowercase();
        if let Some(attribute) = key.strip_prefix("ce-") {
            values.insert(attribute.to_owned(), value.clone());
        }
    }
    let required = |name: &str| {
        values
            .get(name)
            .cloned()
            .ok_or_else(|| CeError::new(format!("/{name}"), "required binary attribute missing"))
    };
    let data = if body.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice(body)
                .map_err(|error| CeError::new("/data", error.to_string()))?,
        )
    };
    let mut extensions = BTreeMap::new();
    for (name, value) in &values {
        if !matches!(
            name.as_str(),
            "specversion"
                | "id"
                | "source"
                | "type"
                | "time"
                | "subject"
                | "datacontenttype"
                | "dataschema"
        ) {
            extensions.insert(name.clone(), Value::String(value.clone()));
        }
    }
    let event = CloudEvent {
        specversion: required("specversion")?,
        id: required("id")?,
        source: required("source")?,
        r#type: required("type")?,
        time: values.get("time").cloned(),
        subject: values.get("subject").cloned(),
        datacontenttype: values.get("datacontenttype").cloned(),
        dataschema: values.get("dataschema").cloned(),
        data,
        extensions,
    };
    event
        .validate()
        .map_err(|error| CeError::new(error.pointer, error.message))?;
    Ok(event)
}
