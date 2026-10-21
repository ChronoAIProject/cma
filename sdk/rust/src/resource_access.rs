//! Current policy inspection and owner share management.
use crate::{Client, IdempotencyKey, Result, resources::positive};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

string_enum! { /// Current organization role for scope presentation.
    pub enum OrganizationRole { Admin => "admin", Member => "member", Viewer => "viewer" }
}
/// One current organization choice. It is not reusable resource authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrganizationChoice {
    /// Canonical organization identity.
    pub id: String,
    /// Current presentation role; later resource requests recheck membership.
    pub role: OrganizationRole,
    /// Optional organization display name.
    pub display_name: Option<String>,
}
/// Current organization choices ordered by canonical identifier.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrganizationChoices {
    /// Available organization scopes.
    pub items: Vec<OrganizationChoice>,
}

string_enum! { /// Organization share mode.
    pub enum ShareMode { View => "view", ViewWrite => "view_write", OrgRoles => "org_roles" }
}
/// Retained resource owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceOwner {
    /// Person attribution or scope.
    Person {
        /// The `subject` field.
        subject: String,
    },
    /// Organization attribution or scope.
    Organization {
        /// The `subject` field.
        subject: String,
    },
}
/// Actual authenticated action attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceActor {
    /// Human attribution or scope.
    Human {
        /// The `subject` field.
        subject: String,
    },
    /// Api attribution or scope.
    Api {
        /// The `subject` field.
        subject: String,
        /// The `delegation_id` field.
        delegation_id: Option<String>,
    },
    /// Trigger attribution or scope.
    Trigger {
        /// The `subject` field.
        subject: String,
        /// The `launch_id` field.
        launch_id: String,
    },
    /// Channel attribution or scope.
    Channel {
        /// The `bridge_id` field.
        bridge_id: String,
        /// The `grant_id` field.
        grant_id: String,
        /// The `platform` field.
        platform: String,
        /// The `chat_id` field.
        chat_id: String,
        /// The `sender_id` field.
        sender_id: String,
    },
}
/// Explicit share scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceSelector {
    /// Agent attribution or scope.
    Agent {
        /// The `agent_id` field.
        agent_id: String,
    },
    /// Sandbox attribution or scope.
    Sandbox {
        /// The `sandbox_id` field.
        sandbox_id: String,
    },
}
/// Owner-selectable new grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShareRequest {
    /// The `target_org_id` field.
    pub target_org_id: String,
    /// The `mode` field.
    pub mode: ShareMode,
}
/// Mode change with the expected current revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateShareRequest {
    /// The `expected_revision` field.
    pub expected_revision: u64,
    /// The `mode` field.
    pub mode: ShareMode,
}
/// Current or revoked owner grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceShare {
    /// The `id` field.
    pub id: String,
    /// The `owner` field.
    pub owner: ResourceOwner,
    /// The `selector` field.
    pub selector: ResourceSelector,
    /// The `target_org_id` field.
    pub target_org_id: String,
    /// The `mode` field.
    pub mode: ShareMode,
    /// The `issuer_actor` field.
    pub issuer_actor: ResourceActor,
    /// The `created_at` field.
    pub created_at: String,
    /// The `revoked_at` field.
    pub revoked_at: Option<String>,
    /// The `revision` field.
    pub revision: u64,
}
/// Immutable response to one idempotent share command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareReceipt {
    /// The `operation_id` field.
    pub operation_id: String,
    /// The `share` field.
    pub share: ResourceShare,
    /// The `actor` field.
    pub actor: ResourceActor,
    /// The `at` field.
    pub at: String,
    /// The `policy_revision` field.
    pub policy_revision: u64,
}
/// Bounded active direct grants; inherited and revoked grants are excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveShareList {
    /// The `items` field.
    pub items: Vec<ResourceShare>,
}
/// Current policy only; every later operation checks its own authority and capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceOwnership {
    /// Personal attribution without revealing another person's identity.
    Person {
        /// Whether the authenticated caller is the retained personal owner.
        is_current_person: bool,
    },
    /// Organization attribution, named through authorized organization choices.
    Organization {
        /// Identity for matching a current organization choice.
        organization_id: String,
    },
}
/// Current policy only; every later operation checks its own authority and capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveAccess {
    /// Owner attribution for a redeemed Agent-session link.
    pub shared_by: Option<ShareIdentity>,
    /// Safe ownership summary; absent on older servers.
    pub owner: Option<ResourceOwnership>,
    /// The `capabilities` field.
    pub capabilities: Vec<String>,
    /// The `revalidate_at` field.
    pub revalidate_at: Option<String>,
    /// The `policy_revision` field.
    pub policy_revision: u64,
}

/// Policy and grant routes on one client.
pub struct ResourceAccess<'a> {
    client: &'a Client,
}
impl Client {
    /// Inspect current permissions or manage personal resource shares.
    pub fn resource_access(&self) -> ResourceAccess<'_> {
        ResourceAccess { client: self }
    }
}
impl ResourceAccess<'_> {
    /// List current organization choices without granting resource access.
    pub async fn organizations(&self) -> Result<OrganizationChoices> {
        self.client.get(&["organizations"], &[]).await
    }

    /// Read current capabilities for an agent, sandbox, workspace or worktree.
    pub async fn effective(&self, kind: &str, id: &str) -> Result<EffectiveAccess> {
        self.client.get(&["resource-access", kind, id], &[]).await
    }
    /// List active direct agent or sandbox grants.
    pub async fn list_shares(&self, kind: &str, id: &str) -> Result<ActiveShareList> {
        self.client.get(&["resource-shares", kind, id], &[]).await
    }
    /// Read a management record, including a revoked grant.
    pub async fn read_share(&self, kind: &str, id: &str, share_id: &str) -> Result<ResourceShare> {
        let share: ResourceShare = self
            .client
            .get(&["resource-shares", kind, id, share_id], &[])
            .await?;
        crate::resources::identity(&share.id, share_id)?;
        Ok(share)
    }
    /// Create an owner grant; reuse the key to recover an uncertain response.
    pub async fn create_share(
        &self,
        kind: &str,
        id: &str,
        body: &CreateShareRequest,
        key: &IdempotencyKey,
    ) -> Result<ShareReceipt> {
        self.client
            .command(
                Method::POST,
                &["resource-shares", kind, id],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await
    }
    /// Change mode under share revision CAS.
    pub async fn update_share(
        &self,
        kind: &str,
        id: &str,
        share_id: &str,
        body: &UpdateShareRequest,
        key: &IdempotencyKey,
    ) -> Result<ShareReceipt> {
        positive(body.expected_revision)?;
        self.client
            .command(
                Method::PATCH,
                &["resource-shares", kind, id, share_id],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await
    }
    /// Revoke a grant under share revision CAS without changing its owner.
    pub async fn revoke_share(
        &self,
        kind: &str,
        id: &str,
        share_id: &str,
        expected_revision: u64,
        key: &IdempotencyKey,
    ) -> Result<ShareReceipt> {
        positive(expected_revision)?;
        let segments = ["resource-shares", kind, id, share_id];
        let query = [("expected_revision", expected_revision.to_string())];
        let mut request = crate::http::Request::command::<()>(
            Method::DELETE,
            &segments,
            None,
            key,
            StatusCode::OK,
        )?;
        request.query = &query;
        self.client.request(request).await
    }
}

/// Audience admitted by an Agent share link after proof of link possession.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentShareAudience {
    /// Any authenticated user holding the link.
    Public,
    /// Only current members of this organization.
    Organization {
        /// Canonical organization identity.
        organization_id: String,
    },
    /// Only these canonical directory identities.
    Users {
        /// Current CMA user subjects, resolved through recipient search.
        subjects: Vec<String>,
    },
}
/// Public identity used in sharing selectors and attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareIdentity {
    /// Canonical user subject.
    pub subject: String,
    /// Current public display name.
    pub display_name: Option<String>,
    /// Current public avatar.
    pub avatar_url: Option<String>,
}
/// Metadata for one revocable Agent-session link. Never contains its secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentShareLink {
    /// Link identity.
    pub id: String,
    /// Shared Agent identity; this grants no parent Sandbox access.
    pub agent_id: String,
    /// Recipient constraint, revalidated on access.
    pub audience: AgentShareAudience,
    /// View or view-write.
    pub mode: ShareMode,
    /// Owner attribution.
    pub shared_by: ShareIdentity,
    /// Creation timestamp.
    pub created_at: String,
    /// Revocation timestamp, if revoked.
    pub revoked_at: Option<String>,
    /// Compare-and-swap revision for revocation.
    pub revision: u64,
    /// Number of users who redeemed this specific link.
    pub accepted_count: u64,
}
/// New link input; reuse the exact request and idempotency key on uncertainty.
#[derive(Clone, Serialize)]
pub struct CreateAgentShareLinkRequest {
    /// Public, organization, or canonical selected users.
    pub audience: AgentShareAudience,
    /// View or view-write (legacy org_roles is not valid for links).
    pub mode: ShareMode,
    /// 32 random bytes encoded as lowercase hex. Keep this in the URL fragment.
    /// Only the SHA-256 digest is persisted by CMA.
    pub token: String,
}
/// Active Agent-session sharing links.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentShareLinks {
    /// Link metadata without secrets.
    pub items: Vec<AgentShareLink>,
}
/// Bounded canonical users available to the owning Agent user.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ShareRecipients {
    /// At most 20 matches.
    pub items: Vec<ShareIdentity>,
}
impl ResourceAccess<'_> {
    /// List active links belonging to one personally owned Agent.
    pub async fn agent_share_links(&self, agent_id: &str) -> Result<AgentShareLinks> {
        self.client
            .get(&["agents", agent_id, "share-links"], &[])
            .await
    }
    /// Create a link; the caller retains the secret, which CMA does not return.
    pub async fn create_agent_share_link(
        &self,
        agent_id: &str,
        body: &CreateAgentShareLinkRequest,
        key: &IdempotencyKey,
    ) -> Result<AgentShareLink> {
        self.client
            .command(
                Method::POST,
                &["agents", agent_id, "share-links"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await
    }
    /// Revoke this link without changing any other grant to the same Agent.
    pub async fn revoke_agent_share_link(
        &self,
        agent_id: &str,
        share_id: &str,
        expected_revision: u64,
        key: &IdempotencyKey,
    ) -> Result<AgentShareLink> {
        positive(expected_revision)?;
        let path = ["agents", agent_id, "share-links", share_id];
        let query = [("expected_revision", expected_revision.to_string())];
        let mut request =
            crate::http::Request::command::<()>(Method::DELETE, &path, None, key, StatusCode::OK)?;
        request.query = &query;
        self.client.request(request).await
    }
    /// Exchange possession for access bound to the current authenticated user.
    /// Repeated redemption is idempotent and does not duplicate recipients.
    pub async fn redeem_agent_share_link(
        &self,
        share_id: &str,
        token: &str,
        key: &IdempotencyKey,
    ) -> Result<AgentShareLink> {
        self.client
            .command(
                Method::POST,
                &["agent-share-links", share_id, "redeem"],
                Some(&serde_json::json!({"token":token})),
                key,
                StatusCode::OK,
            )
            .await
    }
    /// Find canonical recipients by public display name or subject (minimum 2 characters).
    pub async fn share_recipients(&self, agent_id: &str, search: &str) -> Result<ShareRecipients> {
        self.client
            .get(
                &["agents", agent_id, "share-recipients"],
                &[("q", search.to_owned())],
            )
            .await
    }
}
