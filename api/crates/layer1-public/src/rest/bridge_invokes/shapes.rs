//! Safe grant management wire projections. They contain no connector, custody
//! reference, raw credential, resource authorization audit or execution proof.
#![allow(dead_code)]
use super::super::{
    bridges::shapes::BridgeTargetShape,
    resource_access_shapes::{ResourceActorView, ResourceOwnerView},
};
use layer2_core::bridge::{invoke::BridgeSharedContextConsent, BridgeAgentTarget};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueInvokeGrantRequest {
    pub expected_bridge_revision: u64,
    #[schema(value_type=BridgeTargetShape)]
    pub target: BridgeAgentTarget,
    /// Required explicit consent to use the retained Agent's existing context.
    #[schema(value_type=SharedContextConsentShape)]
    pub shared_context: BridgeSharedContextConsent,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeInvokeGrantRequest {
    pub expected_bridge_revision: u64,
    pub expected_revision: u64,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SharedContextConsentShape {
    UseRetainedAgentContext,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvokeGrantConsentShape {
    UseRetainedAgentContext,
    UseChannelCreatedAgent,
}
#[derive(ToSchema)]
pub(crate) struct InvokeGrantPinShape {
    bridge_id: String,
    config_revision: u64,
}
#[derive(ToSchema)]
pub(crate) struct InvokeTargetShape {
    ids: BridgeTargetShape,
    sandbox_created_at: String,
    workspace_created_at: String,
    agent_created_at: String,
    agent_creation_fingerprint: String,
    worktree_identity_digest: String,
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum InvokeGrantStateShape {
    Active,
    Revoked {
        at: String,
        actor: ResourceActorView,
    },
}
#[derive(ToSchema)]
pub(crate) struct InvokeGrantShape {
    id: String,
    revision: u64,
    owner: ResourceOwnerView,
    pin: InvokeGrantPinShape,
    target: InvokeTargetShape,
    consent: InvokeGrantConsentShape,
    issued_at: String,
    issued_by: ResourceActorView,
    state: InvokeGrantStateShape,
}
#[derive(ToSchema)]
pub(crate) struct InvokeReceiptShape {
    id: String,
    request_digest: String,
    bridge_id: String,
    actor: ResourceActorView,
    result: InvokeGrantShape,
}
#[derive(ToSchema)]
pub(crate) struct InvokePageShape {
    items: Vec<InvokeGrantShape>,
    next_cursor: Option<String>,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvokeConsentStatusShape {
    NotApplicable,
    ConsentRequired,
    ConsentPresent,
}
#[derive(ToSchema)]
pub(crate) struct InvokeReadinessShape {
    bridge_id: String,
    revision: u64,
    config_revision: u64,
    status: InvokeConsentStatusShape,
    candidate: Option<InvokeGrantShape>,
}
