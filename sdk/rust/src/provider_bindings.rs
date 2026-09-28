//! Gateway management of provider bindings and external conversations.
use crate::{Client, Result, http::Request};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// Provider target selection. Omission preserves profile provisioning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BindingTarget {
    /// Provision resources from the supplied profile.
    Profile,
    /// Borrow an existing Agent without changing its configuration or lifetime.
    ExistingAgent {
        /// Existing CMA Agent owned by the declared profile owner.
        agent_id: String,
    },
}

/// Safe provider binding metadata without credentials.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderBinding {
    /// Provider resource target; omitted for profile provisioning.
    #[serde(default)]
    pub target: Option<BindingTarget>,
    /// External binding identity.
    pub binding_id: String,
    /// Resource owner.
    pub owner: String,
    /// Canonical profile digest.
    pub profile_digest: String,
    /// Reserved Sandbox.
    pub sandbox_id: String,
    /// Reserved Workspace.
    pub workspace_id: String,
    /// Supported turn contract.
    pub level: String,
    /// Current binding lifecycle state.
    pub state: String,
    /// Optional safe failure reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Profile fields that were applied.
    pub honoured: Vec<String>,
    /// Explicit field refusals.
    pub refused: Vec<BindingRefusal>,
    /// Current credential generation.
    pub generation: u64,
    /// Reservation time.
    pub created_at: String,
    /// Last mutation time.
    pub updated_at: String,
}
/// One refused profile field.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BindingRefusal {
    /// Profile field path.
    pub field: String,
    /// Safe refusal reason.
    pub reason: String,
}
/// External conversation mapping.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BindingConversation {
    /// External conversation identity.
    pub conversation_id: String,
    /// Reserved CMA Agent identity; absent until provider-owned source-user routing selects it.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Current conversation state.
    pub state: String,
}
/// Binding advertisement for provider-side tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingTools {
    /// Native tool endpoint URL.
    pub native_mcp_url: String,
    /// Catalog service slug.
    pub nyxid_service_slug: String,
}
/// Create, replay, or reconfigure a binding. Debug output omits credentials.
#[derive(Clone, Serialize, Deserialize)]
pub struct PutProviderBinding {
    /// Resource target; omitted for profile provisioning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<BindingTarget>,
    /// Protocol schema version, currently 1.
    pub schema_version: u16,
    /// Complete canonical profile snapshot.
    pub profile: serde_json::Value,
    /// SHA-256 of the canonical snapshot.
    pub profile_digest: String,
    /// Scoped agent credential for encrypted custody.
    pub agent_key: String,
    /// Tool capability advertisement.
    pub tools: BindingTools,
}
impl std::fmt::Debug for PutProviderBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PutProviderBinding")
            .field("schema_version", &self.schema_version)
            .field("profile_digest", &self.profile_digest)
            .finish_non_exhaustive()
    }
}
/// Operations requiring both gateway record and provider binding authority.
pub struct ProviderBindings<'a>(&'a Client);
impl Client {
    /// Manage provider bindings as a gateway.
    pub fn provider_bindings(&self) -> ProviderBindings<'_> {
        ProviderBindings(self)
    }
}
impl ProviderBindings<'_> {
    /// Reserve, replay, or reconfigure a binding using its retained identity.
    pub async fn put(&self, id: &str, body: &PutProviderBinding) -> Result<ProviderBinding> {
        self.0
            .operation(
                Method::PUT,
                &["cmaeg", "bindings", id],
                Some(body),
                StatusCode::OK,
            )
            .await
    }
    /// Read safe binding metadata.
    pub async fn get(&self, id: &str) -> Result<ProviderBinding> {
        self.0.get(&["cmaeg", "bindings", id], &[]).await
    }
    /// Retire a binding and its authority.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.0
            .empty(Request::operation::<()>(
                Method::DELETE,
                &["cmaeg", "bindings", id],
                None,
                StatusCode::NO_CONTENT,
            )?)
            .await
    }
    /// Create or replay a deterministic external conversation mapping.
    pub async fn ensure_conversation(
        &self,
        id: &str,
        conversation: &str,
    ) -> Result<BindingConversation> {
        let path = ["cmaeg", "bindings", id, "conversations", conversation];
        let mut request = Request::operation::<()>(Method::PUT, &path, None, StatusCode::CREATED)?;
        request.statuses.push(StatusCode::OK);
        self.0.request(request).await
    }
    /// Retire a mapped conversation.
    pub async fn delete_conversation(&self, id: &str, conversation: &str) -> Result<()> {
        self.0
            .empty(Request::operation::<()>(
                Method::DELETE,
                &["cmaeg", "bindings", id, "conversations", conversation],
                None,
                StatusCode::NO_CONTENT,
            )?)
            .await
    }
}
