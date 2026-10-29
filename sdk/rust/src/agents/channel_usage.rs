//! Explicit channel history is independent of immutable creation provenance.
use super::Agents;
use crate::{Error, PageOptions, Result, resource_access::ResourceActor, resources::identity};
use serde::Deserialize;

/// Safe broad projection without private channel references.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChannelUsageSummary {
    /// At least one currently attached channel binding exists.
    pub active: bool,
    /// A binding or accepted-command event has been retained, including detachments.
    pub has_history: bool,
    /// First accepted channel command; attachment alone does not invent use.
    pub first_used_at: Option<String>,
    /// Last accepted channel command, preserved after detachment.
    pub last_used_at: Option<String>,
}
/// Management-only binding reference and its recorded accepted-command use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChannelBinding {
    /// Immutable binding identity.
    pub binding_id: String,
    /// Owning bridge reference.
    pub bridge_id: String,
    /// Binding partition reference.
    pub partition_id: String,
    /// Positive binding generation.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub generation: u64,
    /// Prior binding for reset/rebind lineage, absent for the first generation.
    pub previous_binding_id: Option<String>,
    /// Original attachment time.
    pub attached_at: String,
    /// First accepted command through this particular binding.
    pub first_used_at: Option<String>,
    /// Last accepted command through this particular binding.
    pub last_used_at: Option<String>,
}
/// Current channel facts under the Agent's current resource authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentChannelUsage {
    /// Exact requested Agent identity.
    pub agent_id: String,
    /// Safe active/history summary, independent of creation category.
    pub channel_usage: ChannelUsageSummary,
    /// Only current resource managers receive binding references.
    pub bindings: Option<Vec<ChannelBinding>>,
}
string_enum! { /// Attributed purpose of a channel-history transition.
    pub enum ChannelBindingChangeReason {
        Attachment => "attachment", Detachment => "detachment", Reset => "reset", Rebind => "rebind",
        AdministrativeIntervention => "administrative_intervention", AcceptedCommand => "accepted_command"
    }
}
/// Explicit binding transition, with full use facts retained in before/after.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelUsageChange {
    /// New attachment.
    Attach {
        /// New binding identity.
        binding_id: String,
    },
    /// An accepted channel command.
    Use {
        /// Binding used by the command.
        binding_id: String,
    },
    /// An explicit detachment.
    Detach {
        /// Detached binding identity.
        binding_id: String,
    },
    /// Atomic reset/rebind of an attached binding.
    Replace {
        /// Outgoing binding identity.
        before_binding_id: String,
        /// Incoming binding identity.
        binding_id: String,
    },
}
/// Manager-only retained event, attributed without exposing store internals.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChannelUsageEvent {
    /// Gap-free positive event revision.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub revision: u64,
    /// Actual recorded actor; historical attribution grants no current authority.
    pub actor: ResourceActor,
    /// Original event timestamp.
    pub at: String,
    /// Outgoing binding's detach/reset/rebind time, otherwise absent.
    pub detached_at: Option<String>,
    /// Recorded reason for the change.
    pub reason: ChannelBindingChangeReason,
    /// Explicit changed binding identities.
    pub change: ChannelUsageChange,
    /// Active bindings immediately before this event, at most 16.
    pub before: Vec<ChannelBinding>,
    /// Active bindings immediately after this event, at most 16.
    pub after: Vec<ChannelBinding>,
}
/// Bounded history under current resource management authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentChannelUsageHistory {
    /// Exact requested Agent identity, including an empty page.
    pub agent_id: String,
    /// Ordered gap-free history entries.
    pub items: Vec<ChannelUsageEvent>,
    /// Exclusive revision cursor for the next page.
    pub next: Option<String>,
}
fn cursor(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= i64::MAX as u64 && n.to_string() == value)
}
fn bindings(items: &[ChannelBinding]) -> Result<()> {
    if items.len() > 16
        || items
            .iter()
            .map(|item| &item.binding_id)
            .collect::<std::collections::HashSet<_>>()
            .len()
            != items.len()
    {
        return Err(Error::protocol(
            "channel bindings exceeded their bounds or repeated an identity",
        ));
    }
    Ok(())
}
impl Agents<'_> {
    /// Read the legacy Bridge usage projection; resource managers receive bindings.
    /// Provider admissions do not populate it. Use `provider_associations` for
    /// retained membership; empty usage does not prove no prior association.
    pub async fn channel_usage(&self, agent_id: &str) -> Result<AgentChannelUsage> {
        let value: AgentChannelUsage = self
            .client
            .get(&["agents", agent_id, "channel-usage"], &[])
            .await?;
        identity(&value.agent_id, agent_id)?;
        let summary = &value.channel_usage;
        let pair = match (&summary.first_used_at, &summary.last_used_at) {
            (None, None) => true,
            (Some(first), Some(last)) => first <= last,
            _ => false,
        };
        if !pair || (!summary.has_history && (summary.active || summary.first_used_at.is_some())) {
            return Err(Error::protocol("channel usage summary is inconsistent"));
        }
        if let Some(items) = &value.bindings {
            bindings(items)?;
            if items.is_empty() == summary.active {
                return Err(Error::protocol(
                    "channel binding summary disagrees with its active records",
                ));
            }
        }
        Ok(value)
    }
    /// Read bounded manager-only history with an exclusive revision cursor.
    pub async fn channel_usage_history(
        &self,
        agent_id: &str,
        options: &PageOptions,
    ) -> Result<AgentChannelUsageHistory> {
        let mut previous = match &options.cursor {
            Some(value) => cursor(value).ok_or_else(|| {
                Error::validation("channel history cursor must be a positive revision")
            })?,
            None => 0,
        };
        if options.limit.is_some_and(|limit| limit == 0 || limit > 200) {
            return Err(Error::validation(
                "channel history limit must be 1 through 200",
            ));
        }
        let value: AgentChannelUsageHistory = self
            .client
            .get(
                &["agents", agent_id, "channel-usage", "history"],
                &options.query(),
            )
            .await?;
        identity(&value.agent_id, agent_id)?;
        if value.items.len() > usize::from(options.limit.unwrap_or(50)) {
            return Err(Error::protocol("channel history exceeded its page limit"));
        }
        for event in &value.items {
            if previous.checked_add(1) != Some(event.revision) {
                return Err(Error::protocol(
                    "channel history revisions are not contiguous",
                ));
            }
            previous = event.revision;
            bindings(&event.before)?;
            bindings(&event.after)?;
            let detached = matches!(
                event.change,
                ChannelUsageChange::Detach { .. } | ChannelUsageChange::Replace { .. }
            );
            if event.detached_at.as_deref() != detached.then_some(event.at.as_str()) {
                return Err(Error::protocol(
                    "channel history detach time is inconsistent",
                ));
            }
        }
        if value
            .next
            .as_ref()
            .is_some_and(|next| cursor(next) != Some(previous) || value.items.is_empty())
        {
            return Err(Error::protocol(
                "channel history continuation did not match its page",
            ));
        }
        Ok(value)
    }
}
