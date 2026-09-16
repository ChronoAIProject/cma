//! Public consent shapes omit authorization proofs and private connector data.
#![allow(dead_code)]
use super::super::{
    bridge_invokes::shapes::{InvokeGrantPinShape, InvokeTargetShape},
    resource_access_shapes::{ResourceActorView, ResourceOwnerView},
};
use utoipa::ToSchema;

#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceControlRequestShape {
    #[schema(minimum = 1, maximum = 9223372036854775807_i64)]
    expected_bridge_revision: u64,
    #[schema(minimum = 1, maximum = 2)]
    expected_grant_revision: u64,
    /// Zero means no policy ever existed. Allow accepts 0 through 254; revoke accepts
    /// 0 through 255. Result revision 256 is reserved for revocation.
    #[schema(minimum = 0, maximum = 255)]
    expected_policy_revision: u64,
    action: SourceControlActionShape,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceControlActionShape {
    AllowSenderCancelAndSteer,
    RevokeSenderControls,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceControlScopeShape {
    SourceSender,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceControlApprovalModeShape {
    DashboardOnly,
}

/// A retained consent revision, not current execution authority or support.
#[derive(ToSchema)]
pub(crate) struct SourceControlPolicyShape {
    id: String,
    grant_id: String,
    owner: ResourceOwnerView,
    pin: InvokeGrantPinShape,
    target: InvokeTargetShape,
    #[schema(minimum = 1, maximum = 256)]
    revision: u64,
    enabled: bool,
    scope: SourceControlScopeShape,
    approval_mode: SourceControlApprovalModeShape,
    updated_at: String,
    updated_by: ResourceActorView,
}
#[derive(ToSchema)]
pub(crate) struct SourceControlStateShape {
    /// Null means no consent policy was ever recorded for this grant.
    #[schema(required = true)]
    policy: Option<SourceControlPolicyShape>,
}
#[derive(ToSchema)]
pub(crate) struct SourceControlReceiptShape {
    id: String,
    request: SourceControlRequestShape,
    result: SourceControlPolicyShape,
}
