//! Allowlisted, bounded metadata for activity records.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const MAX_DETAIL_VALUE: usize = 512;

/// A reviewed metadata field that is safe to persist in an activity record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DetailKey {
    Runtime,
    Model,
    ResourceName,
    Subject,
    ServiceId,
    SkillId,
    Cursor,
    Limit,
    Status,
    Setting,
    CorrelationId,
    AuthorizationGrantId,
    Revision,
    ProfileKind,
    Visibility,
    ActivityChannel,
    RequestId,
    RequestMethod,
    RequestRoute,
    CredentialKind,
    ActorScope,
    ScopeOwner,
    HttpStatus,
    GatewayAuditWriter,
}

impl DetailKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Model => "model",
            Self::ResourceName => "resource_name",
            Self::Subject => "subject",
            Self::ServiceId => "service_id",
            Self::SkillId => "skill_id",
            Self::Cursor => "cursor",
            Self::Limit => "limit",
            Self::Status => "status",
            Self::Setting => "setting",
            Self::CorrelationId => "correlation_id",
            Self::AuthorizationGrantId => "authorization_grant_id",
            Self::Revision => "revision",
            Self::ProfileKind => "profile_kind",
            Self::Visibility => "visibility",
            Self::ActivityChannel => "activity_channel",
            Self::RequestId => "request_id",
            Self::RequestMethod => "request_method",
            Self::RequestRoute => "request_route",
            Self::CredentialKind => "credential_kind",
            Self::ActorScope => "actor_scope",
            Self::ScopeOwner => "scope_owner",
            Self::HttpStatus => "http_status",
            Self::GatewayAuditWriter => "gateway_audit_writer",
        }
    }

    fn from_code(value: &str) -> Option<Self> {
        Some(match value {
            "runtime" => Self::Runtime,
            "model" => Self::Model,
            "resource_name" => Self::ResourceName,
            "subject" => Self::Subject,
            "service_id" => Self::ServiceId,
            "skill_id" => Self::SkillId,
            "cursor" => Self::Cursor,
            "limit" => Self::Limit,
            "status" => Self::Status,
            "setting" => Self::Setting,
            "correlation_id" => Self::CorrelationId,
            "authorization_grant_id" => Self::AuthorizationGrantId,
            "revision" => Self::Revision,
            "profile_kind" => Self::ProfileKind,
            "visibility" => Self::Visibility,
            "activity_channel" => Self::ActivityChannel,
            "request_id" => Self::RequestId,
            "request_method" => Self::RequestMethod,
            "request_route" => Self::RequestRoute,
            "credential_kind" => Self::CredentialKind,
            "actor_scope" => Self::ActorScope,
            "scope_owner" => Self::ScopeOwner,
            "http_status" => Self::HttpStatus,
            "gateway_audit_writer" => Self::GatewayAuditWriter,
            _ => return None,
        })
    }
}

/// Metadata whose keys and value size are constrained before persistence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityDetail(BTreeMap<DetailKey, String>);

impl ActivityDetail {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, key: DetailKey, value: impl Into<String>) {
        let mut value = value.into();
        if value.len() > MAX_DETAIL_VALUE {
            let mut end = MAX_DETAIL_VALUE;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        self.0.insert(key, value);
    }

    pub fn get(&self, key: DetailKey) -> Option<&str> {
        self.0.get(&key).map(String::as_str)
    }

    pub(crate) fn request_summary(&self) -> Self {
        let mut summary = Self::new();
        for key in [
            DetailKey::ActivityChannel,
            DetailKey::RequestId,
            DetailKey::RequestMethod,
            DetailKey::RequestRoute,
            DetailKey::CredentialKind,
            DetailKey::ActorScope,
            DetailKey::HttpStatus,
        ] {
            if let Some(value) = self.get(key) {
                summary.record(key, value);
            }
        }
        summary
    }

    pub fn from_stored(entries: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut detail = Self::new();
        for (key, value) in entries {
            if let Some(key) = DetailKey::from_code(&key) {
                detail.record(key, value);
            } else {
                tracing::warn!(key = %key, "dropped an unrecognized activity-detail key");
            }
        }
        detail
    }
}

impl Serialize for ActivityDetail {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key.as_str(), value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ActivityDetail {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = BTreeMap::<String, String>::deserialize(deserializer)?;
        Ok(Self::from_stored(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_dropped_and_values_are_bounded() {
        let detail = ActivityDetail::from_stored([
            ("resource_name".into(), "é".repeat(400)),
            ("authorization".into(), "secret".into()),
        ]);
        let value = detail.get(DetailKey::ResourceName).unwrap();
        assert!(value.len() <= MAX_DETAIL_VALUE);
        assert!(value.chars().all(|character| character == 'é'));
        assert!(!serde_json::to_string(&detail).unwrap().contains("secret"));
    }
}
