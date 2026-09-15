//! Explicit channel facts never establish channel authority or change creation
//! provenance. Only a trusted, separately authorized bridge transaction may
//! submit these mutations; no public request is deserialized into this port.

use serde::{Deserialize, Serialize};

use crate::resource_access::{ResourceActor, ResourcePolicyFence};
use crate::sandbox::store::SandboxStoreError;
use crate::sandbox::AgentCreationProvenance;

mod service;
mod transition;
pub use service::{AgentChannelUsageReading, AgentChannelUsageService};
pub use transition::{plan_channel_usage, ChannelUsageMutation};

pub const MAX_ACTIVE_CHANNEL_BINDINGS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBindingReference {
    pub binding_id: String,
    pub bridge_id: String,
    pub partition_id: String,
    pub generation: u64,
    pub previous_binding_id: Option<String>,
}

impl ChannelBindingReference {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if !opaque(&self.binding_id)
            || !opaque(&self.bridge_id)
            || !opaque(&self.partition_id)
            || self.generation == 0
            || self.generation > i64::MAX as u64
            || self
                .previous_binding_id
                .as_ref()
                .is_some_and(|id| !opaque(id) || id == &self.binding_id)
            || (self.generation == 1) != self.previous_binding_id.is_none()
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveChannelBinding {
    pub reference: ChannelBindingReference,
    pub attached_at: String,
    pub first_used_at: Option<String>,
    pub last_used_at: Option<String>,
}

/// Safe for an already authorized broad projection: no bridge, partition,
/// binding, sender or external chat identifiers appear here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelUsageSummary {
    pub active: bool,
    pub has_history: bool,
    pub first_used_at: Option<String>,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentChannelUsageState {
    pub schema_version: u16,
    pub creation: AgentCreationProvenance,
    pub revision: u64,
    pub active_bindings: Vec<ActiveChannelBinding>,
    pub first_used_at: Option<String>,
    pub last_used_at: Option<String>,
    pub last_changed_at: Option<String>,
}

impl AgentChannelUsageState {
    pub fn empty(creation: AgentCreationProvenance) -> Self {
        Self {
            schema_version: 1,
            creation,
            revision: 0,
            active_bindings: vec![],
            first_used_at: None,
            last_used_at: None,
            last_changed_at: None,
        }
    }

    pub fn summary(&self) -> ChannelUsageSummary {
        ChannelUsageSummary {
            active: !self.active_bindings.is_empty(),
            has_history: self.revision > 0,
            first_used_at: self.first_used_at.clone(),
            last_used_at: self.last_used_at.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.schema_version != 1
            || self.revision > i64::MAX as u64
            || self.active_bindings.len() > MAX_ACTIVE_CHANNEL_BINDINGS
            || (self.revision == 0) != self.last_changed_at.is_none()
            || (self.revision == 0
                && (!self.active_bindings.is_empty() || self.first_used_at.is_some()))
            || !ordered_usage(
                &self.first_used_at,
                &self.last_used_at,
                &self.creation.agent_created_at,
                self.last_changed_at.as_deref(),
            )
        {
            return Err(corrupt());
        }
        if self
            .last_changed_at
            .as_ref()
            .is_some_and(|at| !ordered(&self.creation.agent_created_at, at))
        {
            return Err(corrupt());
        }
        let mut ids = std::collections::HashSet::new();
        for binding in &self.active_bindings {
            binding.reference.validate().map_err(|_| corrupt())?;
            if !ids.insert(&binding.reference.binding_id)
                || !ordered(&self.creation.agent_created_at, &binding.attached_at)
                || !ordered_usage(
                    &binding.first_used_at,
                    &binding.last_used_at,
                    &binding.attached_at,
                    self.last_changed_at.as_deref(),
                )
                || self
                    .last_changed_at
                    .as_ref()
                    .is_none_or(|last| !ordered(&binding.attached_at, last))
                || binding.first_used_at.as_ref().is_some_and(|first| {
                    self.first_used_at
                        .as_ref()
                        .is_none_or(|all| !ordered(all, first))
                })
                || binding.last_used_at.as_ref().is_some_and(|last| {
                    self.last_used_at
                        .as_ref()
                        .is_none_or(|all| !ordered(last, all))
                })
            {
                return Err(corrupt());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelBindingChangeReason {
    Attachment,
    Detachment,
    Reset,
    Rebind,
    AdministrativeIntervention,
    AcceptedCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChannelUsageChange {
    Attach {
        binding: ChannelBindingReference,
    },
    Use {
        binding_id: String,
    },
    Detach {
        binding_id: String,
    },
    Replace {
        before_binding_id: String,
        binding: ChannelBindingReference,
    },
}

impl ChannelUsageChange {
    pub fn introduced_binding(&self) -> Option<&ChannelBindingReference> {
        match self {
            Self::Attach { binding } | Self::Replace { binding, .. } => Some(binding),
            _ => None,
        }
    }
}

/// Immutable, paginated audit evidence. Both snapshots are bounded; historical
/// events are separate records and are never truncated when a binding detaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelUsageEvent {
    pub operation_id: String,
    pub fingerprint: String,
    pub actor: ResourceActor,
    pub at: String,
    /// Detach/reset/rebind time for the outgoing binding. Its reference and
    /// first/last use remain in `before`, even after the bridge is retired.
    pub detached_at: Option<String>,
    pub reason: ChannelBindingChangeReason,
    pub change: ChannelUsageChange,
    pub predecessor: Option<ChannelBindingReference>,
    pub before: AgentChannelUsageState,
    pub after: AgentChannelUsageState,
}

impl ChannelUsageEvent {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if !digest(&self.operation_id) || !digest(&self.fingerprint) {
            return Err(corrupt());
        }
        let detached = matches!(
            self.change,
            ChannelUsageChange::Detach { .. } | ChannelUsageChange::Replace { .. }
        );
        if self.detached_at.as_deref() != detached.then_some(self.at.as_str()) {
            return Err(corrupt());
        }
        if self.fingerprint
            != hash(&(
                "channel-usage-input-v1",
                &self.before.creation,
                self.before.revision,
                &self.actor,
                &self.change,
                self.reason,
            ))
        {
            return Err(corrupt());
        }
        self.before.validate()?;
        self.after.validate()?;
        let after = transition::apply(
            &self.before,
            &self.change,
            &self.at,
            self.reason,
            self.predecessor.as_ref(),
        )
        .map_err(|_| corrupt())?;
        if after != self.after || !valid_actor(&self.actor) {
            return Err(corrupt());
        }
        if let ResourceActor::Channel { bridge_id, .. } = &self.actor {
            let binding = match &self.change {
                ChannelUsageChange::Attach { binding }
                | ChannelUsageChange::Replace { binding, .. } => Some(binding),
                ChannelUsageChange::Use { binding_id }
                | ChannelUsageChange::Detach { binding_id } => self
                    .before
                    .active_bindings
                    .iter()
                    .find(|active| &active.reference.binding_id == binding_id)
                    .map(|active| &active.reference),
            };
            if binding.is_none_or(|binding| &binding.bridge_id != bridge_id) {
                return Err(corrupt());
            }
        }
        Ok(())
    }
    pub fn digest(&self) -> String {
        hash(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAgentChannelUsage {
    pub state: AgentChannelUsageState,
    pub latest_event_digest: String,
}

impl StoredAgentChannelUsage {
    pub fn validate_latest(&self, event: &ChannelUsageEvent) -> Result<(), SandboxStoreError> {
        event.validate()?;
        if self.state != event.after || self.latest_event_digest != event.digest() {
            return Err(corrupt());
        }
        Ok(())
    }
}

fn valid_actor(actor: &ResourceActor) -> bool {
    match actor {
        ResourceActor::Human { subject } => bounded(subject.as_str()),
        ResourceActor::Api {
            subject,
            delegation_id,
        } => bounded(subject.as_str()) && delegation_id.as_ref().is_none_or(|id| bounded(id)),
        ResourceActor::Channel {
            bridge_id,
            grant_id,
            platform,
            chat_id,
            sender_id,
        } => [bridge_id, grant_id, platform, chat_id, sender_id]
            .into_iter()
            .all(|s| bounded(s)),
        ResourceActor::Trigger { .. } => false,
    }
}
fn bounded(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn opaque(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.:".contains(&c))
}
fn ordered(first: &str, last: &str) -> bool {
    match (
        crate::seconds_from_rfc3339(first),
        crate::seconds_from_rfc3339(last),
    ) {
        (Some(a), Some(b)) => a <= b,
        _ => false,
    }
}
fn ordered_usage(
    first: &Option<String>,
    last: &Option<String>,
    start: &str,
    end: Option<&str>,
) -> bool {
    match (first, last) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            ordered(start, a) && ordered(a, b) && end.is_some_and(|end| ordered(b, end))
        }
        _ => false,
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn hash(value: &impl Serialize) -> String {
    crate::profiles::content_digest(value).expect("typed usage facts serialize")
}
fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("invalid channel usage facts")
}
fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("channel usage evidence is inconsistent".into())
}
