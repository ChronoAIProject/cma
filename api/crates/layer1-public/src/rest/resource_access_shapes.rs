use layer2_core::resource_access::{
    ResourceActor, ResourceOwner, ResourceSelector, ResourceShare, ShareMode, ShareMutationReceipt,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ShareModeView {
    View,
    ViewWrite,
    OrgRoles,
}
impl From<ShareModeView> for ShareMode {
    fn from(value: ShareModeView) -> Self {
        match value {
            ShareModeView::View => Self::View,
            ShareModeView::ViewWrite => Self::ViewWrite,
            ShareModeView::OrgRoles => Self::OrgRoles,
        }
    }
}
impl From<ShareMode> for ShareModeView {
    fn from(value: ShareMode) -> Self {
        match value {
            ShareMode::View => Self::View,
            ShareMode::ViewWrite => Self::ViewWrite,
            ShareMode::OrgRoles => Self::OrgRoles,
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateShareBody {
    pub target_org_id: String,
    pub mode: ShareModeView,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateShareBody {
    pub expected_revision: u64,
    pub mode: ShareModeView,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeShareQuery {
    pub expected_revision: u64,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResourceOwnerView {
    Person { subject: String },
    Organization { subject: String },
}
impl From<ResourceOwner> for ResourceOwnerView {
    fn from(value: ResourceOwner) -> Self {
        match value {
            ResourceOwner::Person { subject } => Self::Person {
                subject: subject.to_string(),
            },
            ResourceOwner::Organization { subject } => Self::Organization {
                subject: subject.to_string(),
            },
        }
    }
}
#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResourceActorView {
    Human {
        subject: String,
    },
    Api {
        subject: String,
        delegation_id: Option<String>,
    },
    Trigger {
        subject: String,
        launch_id: String,
    },
    Channel {
        bridge_id: String,
        grant_id: String,
        platform: String,
        chat_id: String,
        sender_id: String,
    },
}
impl From<ResourceActor> for ResourceActorView {
    fn from(value: ResourceActor) -> Self {
        match value {
            ResourceActor::Human { subject } => Self::Human {
                subject: subject.to_string(),
            },
            ResourceActor::Api {
                subject,
                delegation_id,
            } => Self::Api {
                subject: subject.to_string(),
                delegation_id,
            },
            ResourceActor::Trigger { subject, launch_id } => Self::Trigger {
                subject: subject.to_string(),
                launch_id: launch_id.to_string(),
            },
            ResourceActor::Channel {
                bridge_id,
                grant_id,
                platform,
                chat_id,
                sender_id,
            } => Self::Channel {
                bridge_id,
                grant_id,
                platform,
                chat_id,
                sender_id,
            },
        }
    }
}
#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResourceSelectorView {
    Agent { agent_id: String },
    Sandbox { sandbox_id: String },
}
impl From<ResourceSelector> for ResourceSelectorView {
    fn from(value: ResourceSelector) -> Self {
        match value {
            ResourceSelector::Agent { agent_id } => Self::Agent {
                agent_id: agent_id.to_string(),
            },
            ResourceSelector::Sandbox { sandbox_id } => Self::Sandbox {
                sandbox_id: sandbox_id.to_string(),
            },
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct ResourceShareView {
    pub id: String,
    pub owner: ResourceOwnerView,
    pub selector: ResourceSelectorView,
    pub target_org_id: String,
    pub mode: ShareModeView,
    pub issuer_actor: ResourceActorView,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub revision: u64,
}
impl From<ResourceShare> for ResourceShareView {
    fn from(value: ResourceShare) -> Self {
        Self {
            id: value.id,
            owner: value.owner.into(),
            selector: value.selector.into(),
            target_org_id: value.target_org_id.to_string(),
            mode: value.mode.into(),
            issuer_actor: value.issuer_actor.into(),
            created_at: value.created_at,
            revoked_at: value.revoked_at,
            revision: value.revision,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct ShareReceiptView {
    pub operation_id: String,
    pub share: ResourceShareView,
    pub actor: ResourceActorView,
    pub at: String,
    pub policy_revision: u64,
}
impl From<ShareMutationReceipt> for ShareReceiptView {
    fn from(value: ShareMutationReceipt) -> Self {
        Self {
            operation_id: value.operation_id,
            share: value.share.into(),
            actor: value.actor.into(),
            at: value.at,
            policy_revision: value.policy_revision,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct ActiveShareList {
    pub items: Vec<ResourceShareView>,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResourceOwnershipView {
    /// Personal owner identity is private to the owner.
    Person { is_current_person: bool },
    /// Resolve the name only through the caller's authorized organization choices.
    Organization { organization_id: String },
}

#[derive(Serialize, ToSchema)]
pub(crate) struct EffectiveAccessView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_by: Option<super::agent_share_links::ShareIdentityView>,
    /// Safe ownership attribution without parent identities or private person IDs.
    pub owner: ResourceOwnershipView,
    /// Current policy permissions; individual operations also check lifecycle
    /// and runtime support. This document cannot authorize a later operation.
    pub capabilities: Vec<String>,
    pub revalidate_at: Option<String>,
    pub policy_revision: u64,
}
