use super::super::{BridgeInvokePin, BridgeInvokeTarget};
use crate::resource_access::{ResourceActor, ResourceOwner};
use serde::{Deserialize, Deserializer, Serialize};

string_enum! { /// Separate original-sender consent action; no runtime control is executed.
    pub enum BridgeSourceControlAction { AllowSenderCancelAndSteer => "allow_sender_cancel_and_steer", RevokeSenderControls => "revoke_sender_controls" }
}
string_enum! { /// Consent is limited to the original channel sender.
    pub enum BridgeSourceControlScope { SourceSender => "source_sender" }
}
string_enum! { /// Approval resolution remains on the dashboard.
    pub enum BridgeSourceControlApprovalMode { DashboardOnly => "dashboard_only" }
}
/// Exact consent request. Preserve all fields and the key for uncertain-result recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSourceControlRequest {
    /// Current Bridge revision, positive and bounded by signed 64-bit storage.
    pub expected_bridge_revision: u64,
    /// Current invoke grant revision: one active, two revoked.
    pub expected_grant_revision: u64,
    /// Zero means absent. Allow accepts 0..=254; revoke accepts 0..=255.
    pub expected_policy_revision: u64,
    /// Explicit grant or revocation of original-sender consent.
    pub action: BridgeSourceControlAction,
}
/// Retained consent policy. Enabled alone proves no current runtime capability.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSourceControlPolicy {
    /// Stable brscp_ plus lowercase SHA-256 identity.
    pub id: String,
    /// Exact invoke grant to which this policy belongs.
    pub grant_id: String,
    /// Retained resource owner.
    #[serde(deserialize_with = "owner")]
    pub owner: ResourceOwner,
    /// Original Bridge configuration generation.
    pub pin: BridgeInvokePin,
    /// Original exact Agent creation lineage.
    pub target: BridgeInvokeTarget,
    /// Consent revision,1..=256;256 must be revoked.
    pub revision: u64,
    /// Explicitly recorded consent, independent of current support and authority.
    pub enabled: bool,
    /// Always original source sender.
    pub scope: BridgeSourceControlScope,
    /// Always dashboard-only approvals.
    pub approval_mode: BridgeSourceControlApprovalMode,
    /// Time this consent revision was recorded.
    pub updated_at: String,
    /// Actual authenticated human or nondelegated API actor.
    #[serde(deserialize_with = "manager")]
    pub updated_by: ResourceActor,
}
/// Current consent observation. Absence is a required null, never an omitted field.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSourceControlState {
    /// None means no policy was recorded; disabled policies retain their own revision.
    #[serde(deserialize_with = "required_nullable")]
    pub policy: Option<BridgeSourceControlPolicy>,
}
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}
/// Immutable acceptance of one exact consent command. Recovery does not reapply it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeSourceControlReceipt {
    /// brscc_ plus lowercase SHA-256 receipt identity.
    pub id: String,
    /// Exact original request, even when current policy has since changed.
    pub request: BridgeSourceControlRequest,
    /// Policy revision originally committed by this command.
    pub result: BridgeSourceControlPolicy,
}

fn owner<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ResourceOwner, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Owner {
        Person { subject: String },
        Organization { subject: String },
    }
    Ok(match Owner::deserialize(deserializer)? {
        Owner::Person { subject } => ResourceOwner::Person { subject },
        Owner::Organization { subject } => ResourceOwner::Organization { subject },
    })
}
fn manager<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<ResourceActor, D::Error> {
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum Manager {
        Human {
            subject: String,
        },
        Api {
            subject: String,
            #[serde(deserialize_with = "required_nullable")]
            delegation_id: Option<String>,
        },
    }
    Ok(match Manager::deserialize(deserializer)? {
        Manager::Human { subject } => ResourceActor::Human { subject },
        Manager::Api {
            subject,
            delegation_id,
        } => ResourceActor::Api {
            subject,
            delegation_id,
        },
    })
}
