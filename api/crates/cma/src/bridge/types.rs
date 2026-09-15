use layer2_core::bridge::{BridgeError, BridgePlatform, BridgeSetupBlock};
use layer2_core::resource_access::ResourceOwner;
use layer2_core::SubjectId;
use serde::Deserialize;

/// Minimal projections of NyxID 85607255ee0beadca460a350e0aa6d177edd2fd2.
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
    pub platform: BridgePlatform,
    pub is_active: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Keys {
    pub keys: Vec<Key>,
}
#[derive(Deserialize)]
pub(super) struct Key {
    pub id: String,
    pub is_active: bool,
    pub expires_at: Option<String>,
    pub state_version: i64,
    pub scopes: String,
    pub allowed_service_ids: Vec<String>,
    pub allow_all_services: bool,
    pub allowed_node_ids: Vec<String>,
    pub allow_all_nodes: bool,
    pub platform: Option<BridgePlatform>,
    pub callback_url: Option<String>,
    pub credential_source: CredentialSource,
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
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum CredentialSource {
    Personal,
    Org {
        org_id: String,
        role: Role,
        allowed: bool,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Role {
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
