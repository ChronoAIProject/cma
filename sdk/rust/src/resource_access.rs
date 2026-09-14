//! Current policy inspection and owner share management.
use crate::{Client, IdempotencyKey, Result, resources::positive};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

string_enum! { /// Organization share mode.
    pub enum ShareMode { View => "view", OrgRoles => "org_roles" }
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
pub struct EffectiveAccess {
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
