use super::super::resource_access_shapes::ResourceActorView;
use layer2_core::sandbox::channel_usage::*;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub(crate) struct ChannelUsageSummaryView {
    active: bool,
    has_history: bool,
    first_used_at: Option<String>,
    last_used_at: Option<String>,
}
impl From<ChannelUsageSummary> for ChannelUsageSummaryView {
    fn from(value: ChannelUsageSummary) -> Self {
        Self {
            active: value.active,
            has_history: value.has_history,
            first_used_at: value.first_used_at,
            last_used_at: value.last_used_at,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct ChannelBindingView {
    binding_id: String,
    bridge_id: String,
    partition_id: String,
    generation: u64,
    previous_binding_id: Option<String>,
    attached_at: String,
    first_used_at: Option<String>,
    last_used_at: Option<String>,
}
impl From<ActiveChannelBinding> for ChannelBindingView {
    fn from(value: ActiveChannelBinding) -> Self {
        Self {
            binding_id: value.reference.binding_id,
            bridge_id: value.reference.bridge_id,
            partition_id: value.reference.partition_id,
            generation: value.reference.generation,
            previous_binding_id: value.reference.previous_binding_id,
            attached_at: value.attached_at,
            first_used_at: value.first_used_at,
            last_used_at: value.last_used_at,
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct AgentChannelUsageView {
    agent_id: String,
    channel_usage: ChannelUsageSummaryView,
    /// Present only with current resource management authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    bindings: Option<Vec<ChannelBindingView>>,
}
impl From<AgentChannelUsageReading> for AgentChannelUsageView {
    fn from(value: AgentChannelUsageReading) -> Self {
        Self {
            agent_id: value.agent_id.to_string(),
            channel_usage: value.summary.into(),
            bindings: value
                .active_bindings
                .map(|items| items.into_iter().map(Into::into).collect()),
        }
    }
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelBindingChangeReasonView {
    Attachment,
    Detachment,
    Reset,
    Rebind,
    AdministrativeIntervention,
    AcceptedCommand,
}
impl From<ChannelBindingChangeReason> for ChannelBindingChangeReasonView {
    fn from(value: ChannelBindingChangeReason) -> Self {
        match value {
            ChannelBindingChangeReason::Attachment => Self::Attachment,
            ChannelBindingChangeReason::Detachment => Self::Detachment,
            ChannelBindingChangeReason::Reset => Self::Reset,
            ChannelBindingChangeReason::Rebind => Self::Rebind,
            ChannelBindingChangeReason::AdministrativeIntervention => {
                Self::AdministrativeIntervention
            }
            ChannelBindingChangeReason::AcceptedCommand => Self::AcceptedCommand,
        }
    }
}
#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ChannelUsageChangeView {
    Attach {
        binding_id: String,
    },
    Use {
        binding_id: String,
    },
    Detach {
        binding_id: String,
    },
    Replace {
        before_binding_id: String,
        binding_id: String,
    },
}
impl From<ChannelUsageChange> for ChannelUsageChangeView {
    fn from(value: ChannelUsageChange) -> Self {
        match value {
            ChannelUsageChange::Attach { binding } => Self::Attach {
                binding_id: binding.binding_id,
            },
            ChannelUsageChange::Use { binding_id } => Self::Use { binding_id },
            ChannelUsageChange::Detach { binding_id } => Self::Detach { binding_id },
            ChannelUsageChange::Replace {
                before_binding_id,
                binding,
            } => Self::Replace {
                before_binding_id,
                binding_id: binding.binding_id,
            },
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct ChannelUsageEventView {
    revision: u64,
    actor: ResourceActorView,
    at: String,
    detached_at: Option<String>,
    reason: ChannelBindingChangeReasonView,
    change: ChannelUsageChangeView,
    before: Vec<ChannelBindingView>,
    after: Vec<ChannelBindingView>,
}
impl From<ChannelUsageEvent> for ChannelUsageEventView {
    fn from(value: ChannelUsageEvent) -> Self {
        Self {
            revision: value.after.revision,
            actor: value.actor.into(),
            at: value.at,
            detached_at: value.detached_at,
            reason: value.reason.into(),
            change: value.change.into(),
            before: value
                .before
                .active_bindings
                .into_iter()
                .map(Into::into)
                .collect(),
            after: value
                .after
                .active_bindings
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub(crate) struct AgentChannelUsageHistoryView {
    pub agent_id: String,
    pub items: Vec<ChannelUsageEventView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
