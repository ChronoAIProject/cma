//! Additive stored browser authority shapes. Legacy active rows retain subject.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BrowserLoginDoc {
    #[serde(rename = "_id")]
    pub(super) id: Binary,
    pub(super) document_schema_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) linked_session: Option<Binary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) binding_id: Option<Binary>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) revoked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) explicit_started_at: Option<DateTime>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) preference_revoked: bool,
    pub(super) expires_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LoginAttemptDoc {
    #[serde(rename = "_id")]
    pub(super) id: Binary,
    pub(super) document_schema_version: i64,
    pub(super) browser_binding: Binary,
    pub(super) verifier: CipherEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) return_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) actor_link_intent: Option<CipherEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) service_authorization: Option<CipherEnvelope>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) silent: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) consent_requested: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) custody_reserved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) started_at: Option<DateTime>,
    pub(super) expires_at: DateTime,
}
