use layer2_core::bridge::{BridgeError, BridgeSetupBlock};
use layer2_core::resource_access::ResourceOwner;
use layer2_core::SubjectId;
use serde::{Deserialize, Serialize};

/// Metadata projections of NyxID 9a3938364a11ab65f3b341e69e2b8c124aefba81.
/// Unknown row fields are discarded; lists must use the complete, unpaginated
/// envelope documented at that revision. Extra pagination metadata fails closed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bots {
    pub bots: Vec<Bot>,
    pub total: u64,
}
#[derive(Deserialize)]
pub(super) struct Bot {
    pub id: String,
    pub user_id: String,
    pub platform: String,
    pub is_active: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Keys {
    pub keys: Vec<Key>,
}
#[derive(Deserialize)]
pub(super) struct Key {
    pub state_version: i64,
    #[serde(flatten)]
    pub authority: KeyAuthority,
}

/// Version-one custody projection. Usage timestamps and the provider's mutable
/// row counter are observations, not part of the retained permission snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeyAuthority {
    pub id: String,
    pub is_active: bool,
    pub expires_at: Option<String>,
    pub rotation_predecessor_id: Option<String>,
    pub scopes: String,
    pub allowed_service_ids: Vec<String>,
    pub allow_all_services: bool,
    pub allow_auto_connected_services: bool,
    pub allowed_node_ids: Vec<String>,
    pub allow_all_nodes: bool,
    pub rate_limit_per_second: Option<u32>,
    pub rate_limit_burst: Option<u32>,
    pub platform: Option<String>,
    pub callback_url: Option<String>,
    pub credential_source: CredentialSource,
    pub purpose: String,
    pub scheduled_write_enabled: bool,
    // NyxID omits this field exactly when there are zero bindings. Nonzero
    // binding contents are not exposed in key metadata and cannot be pinned.
    #[serde(default)]
    pub bindings_count: u64,
}

#[derive(Deserialize)]
pub(super) struct Account {
    pub id: String,
    pub is_active: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingProbe {
    pub messages: Vec<serde::de::IgnoredAny>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CredentialSource {
    Personal,
    Org {
        org_id: String,
        role: Role,
        allowed: bool,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Admin,
    Member,
    Viewer,
}

impl CredentialSource {
    pub fn require_owner(
        &self,
        owner: &ResourceOwner,
        credential_subject: &SubjectId,
    ) -> Result<(), BridgeError> {
        let owner_subject = owner_subject(owner);
        let matches = match self {
            Self::Personal => owner_subject == credential_subject,
            Self::Org {
                org_id,
                role: Role::Admin,
                allowed: true,
            } => {
                matches!(owner, ResourceOwner::Organization { subject } if subject.as_str() == org_id)
            }
            _ => false,
        };
        matches.then_some(()).ok_or_else(binding_mismatch)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Routes {
    pub conversations: Vec<Route>,
    pub total: u64,
}
#[derive(Deserialize)]
pub(super) struct Route {
    pub id: String,
    pub channel_bot_id: Option<String>,
    /// Device routes can share a key and must remain visible to the conflict
    /// scan, even though device bridges are not an admitted platform here.
    pub platform: String,
    pub agent_api_key_id: String,
    pub is_active: bool,
}

pub(super) fn owner_subject(owner: &ResourceOwner) -> &SubjectId {
    match owner {
        ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
    }
}

pub(super) fn binding_mismatch() -> BridgeError {
    BridgeError::ConnectionRequired(BridgeSetupBlock::BindingMismatch)
}

pub(super) fn canonical_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
