//! Content-free evidence and fail-closed sandbox inactivity decisions.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId};
use crate::{seconds_from_rfc3339, MAX_PAUSE_AFTER_SECONDS, MIN_PAUSE_AFTER_SECONDS};

use super::store::SandboxMaintenanceOutcome;
use super::{Agent, AgentLogEntry, AgentStatus, Sandbox, SandboxModelError, SandboxStatus};

static OPEN_DESKTOP_SESSIONS: OnceLock<Mutex<BTreeSet<SandboxId>>> = OnceLock::new();

/// Record a live desktop stream so maintenance cannot classify its Sandbox as
/// idle while the stream is open. The connection's Drop implementation removes
/// the marker; durable command evidence remains the authority for user input.
pub fn register_open_desktop_session(sandbox_id: &SandboxId) {
    OPEN_DESKTOP_SESSIONS
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(sandbox_id.clone());
}

/// Remove the process-local desktop marker after a stream closes.
pub fn unregister_open_desktop_session(sandbox_id: &SandboxId) {
    if let Some(sessions) = OPEN_DESKTOP_SESSIONS.get() {
        sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(sandbox_id);
    }
}

/// Check whether this CMA process currently owns an open desktop stream.
pub fn has_open_desktop_session(sandbox_id: &SandboxId) -> bool {
    OPEN_DESKTOP_SESSIONS.get().is_some_and(|sessions| {
        sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(sandbox_id)
    })
}

/// The newest durable log position without copying its potentially private payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxAgentLogActivity {
    pub agent_id: AgentId,
    pub seq: u64,
    pub appended_at: String,
}

/// Everything needed to decide whether one live agent is quiet, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxAgentActivity {
    pub sandbox_id: SandboxId,
    pub agent_id: AgentId,
    pub status: AgentStatus,
    pub conversation_id: Option<String>,
    pub runtime_generation: Option<u64>,
    pub last_seq: u64,
    pub active_turn: Option<u64>,
    pub work_pending: bool,
    pub stop_requested: bool,
    pub queued_messages_present: bool,
    /// Durable user/channel/trigger activity observed outside the Agent log.
    /// The projection remains content-free; any positive value blocks pause.
    #[serde(default)]
    pub external_activity_present: bool,
    /// Open terminal or desktop leases associated with this Sandbox.
    #[serde(default)]
    pub open_terminal_or_desktop_session: bool,
    pub newest_log: Option<SandboxAgentLogActivity>,
}

impl SandboxAgentActivity {
    /// Project one live Agent and its newest durable log without message content.
    pub fn from_live_agent(
        agent: &Agent,
        newest_log: Option<&AgentLogEntry>,
    ) -> Result<Self, SandboxModelError> {
        if !agent.status.is_live() {
            return Err(SandboxModelError::invalid_value(
                "sandbox.inactivity.live_agent",
            ));
        }
        Ok(Self {
            sandbox_id: agent.sandbox_id.clone(),
            agent_id: agent.id.clone(),
            status: agent.status,
            conversation_id: agent.conversation.conversation_id.clone(),
            runtime_generation: agent.conversation.runtime_generation,
            last_seq: agent.conversation.last_seq,
            active_turn: agent.conversation.active_turn,
            work_pending: agent.work.step.is_pending(),
            stop_requested: agent.requested_stop_reason.is_some(),
            queued_messages_present: !agent.conversation.queued_messages.is_empty(),
            external_activity_present: false,
            open_terminal_or_desktop_session: false,
            newest_log: newest_log.map(|entry| SandboxAgentLogActivity {
                agent_id: entry.agent_id.clone(),
                seq: entry.seq,
                appended_at: entry.appended_at.clone(),
            }),
        })
    }
}

/// Fixed and content-free refusal from one inactivity evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InactivityPauseRefusal {
    AlwaysOn,
    PendingControl,
    LegacyLogAgentCount,
    LegacyLogExpectedRows,
    LegacyLogRetainedRows,
    LegacyLogBytes,
    LegacyLogProofPending,
    LegacyLogProofByteLimit,
    OpenSession,
    SessionEvidenceUnclear,
    PendingWorkspaceWork,
    SandboxNotActive,
    InvalidRuntime,
    InvalidActivityTime,
    RecentSandboxActivity,
    RecentRuntimeActivity,
    RecentWorkspaceActivity,
    NoLiveAgents,
    InvalidAgentSet,
    AgentNotQuiet,
    MissingConversation,
    PendingAgentWork,
    PendingCommand,
    MissingLogEvidence,
    InvalidLogEvidence,
    InvalidStoredEvidence,
    RecentAgentActivity,
    PendingLegacyReuse,
    PolicyChanged,
    PolicyGrace,
    SpendStopLifted,
    EvidenceUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InactivityPausePreview {
    pub outcome: SandboxMaintenanceOutcome,
    pub refusal: Option<InactivityPauseRefusal>,
}

use InactivityPauseRefusal as Refusal;

/// Decide from one complete projection of all live Agents at a single store read.
pub fn is_inactivity_pause_candidate(
    sandbox: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    now: &str,
    pause_after_seconds: u64,
) -> bool {
    inactivity_pause_refusal(sandbox, live_agents, now, pause_after_seconds).is_none()
}

pub(super) fn inactivity_pause_refusal(
    sandbox: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    now: &str,
    pause_after_seconds: u64,
) -> Option<InactivityPauseRefusal> {
    let Some((now, at)) =
        seconds_from_rfc3339(now).zip(seconds_from_rfc3339(&sandbox.last_activity_at))
    else {
        return Some(Refusal::InvalidActivityTime);
    };
    if !(MIN_PAUSE_AFTER_SECONDS..=MAX_PAUSE_AFTER_SECONDS).contains(&pause_after_seconds) {
        return Some(Refusal::PolicyChanged);
    }
    if now <= at.saturating_add(pause_after_seconds) {
        return Some(Refusal::RecentSandboxActivity);
    }
    quiet_pause_refusal(sandbox, live_agents, now, pause_after_seconds)
}

/// Quiet-pause eligibility without an inactivity threshold bound.
pub fn is_quiet_pause_candidate(
    sandbox: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    now: &str,
    idle_after_seconds: u64,
) -> bool {
    seconds_from_rfc3339(now).is_some_and(|now| {
        quiet_pause_refusal(sandbox, live_agents, now, idle_after_seconds).is_none()
    })
}

fn quiet_pause_refusal(
    sandbox: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    now: u64,
    idle_after_seconds: u64,
) -> Option<InactivityPauseRefusal> {
    if sandbox.status != SandboxStatus::Active {
        return Some(Refusal::SandboxNotActive);
    }
    if !sandbox.runtime.as_ref().is_some_and(|runtime| {
        !runtime.runtime_id.trim().is_empty()
            && !runtime.runtime_version.trim().is_empty()
            && seconds_from_rfc3339(&runtime.bound_at).is_some()
    }) {
        return Some(Refusal::InvalidRuntime);
    }
    if live_agents.is_empty() {
        return Some(Refusal::NoLiveAgents);
    }
    let mut agent_ids = BTreeSet::new();
    for activity in live_agents {
        if activity.sandbox_id != sandbox.id || !agent_ids.insert(&activity.agent_id) {
            return Some(Refusal::InvalidAgentSet);
        }
        if let Some(refusal) = agent_pause_refusal(activity, now, idle_after_seconds) {
            return Some(refusal);
        }
    }
    None
}

fn agent_pause_refusal(
    activity: &SandboxAgentActivity,
    now: u64,
    idle_after_seconds: u64,
) -> Option<InactivityPauseRefusal> {
    if !matches!(activity.status, AgentStatus::Ready | AgentStatus::Paused) {
        return Some(Refusal::AgentNotQuiet);
    }
    if !complete_conversation_identity(activity) {
        return Some(Refusal::MissingConversation);
    }
    if activity.active_turn.is_some()
        || activity.work_pending
        || activity.stop_requested
        || activity.queued_messages_present
    {
        return Some(Refusal::PendingAgentWork);
    }
    if activity.external_activity_present {
        return Some(Refusal::PendingCommand);
    }
    if activity.open_terminal_or_desktop_session {
        return Some(Refusal::OpenSession);
    }
    let Some(newest) = &activity.newest_log else {
        return Some(Refusal::MissingLogEvidence);
    };
    let Some(appended_at) = seconds_from_rfc3339(&newest.appended_at) else {
        return Some(Refusal::InvalidLogEvidence);
    };
    if newest.agent_id != activity.agent_id || newest.seq != activity.last_seq {
        return Some(Refusal::InvalidLogEvidence);
    }
    (now <= appended_at.saturating_add(idle_after_seconds)).then_some(Refusal::RecentAgentActivity)
}

/// Pause eligibility under a platform spend stop (decision 26). Every live
/// Agent must be quiet exactly as for inactivity, with a zero threshold; an
/// Active Sandbox without live Agents is a candidate too. Keep-alive
/// preferences are the stores' to ignore for this cause; the stores also
/// refuse a Sandbox with pending Workspace work or, without live Agents, an
/// open owner session.
pub fn is_platform_stop_pause_candidate(
    sandbox: &Sandbox,
    live_agents: &[SandboxAgentActivity],
    now: &str,
) -> bool {
    if !live_agents.is_empty() {
        return is_quiet_pause_candidate(sandbox, live_agents, now, 0);
    }
    seconds_from_rfc3339(now).is_some()
        && sandbox.status == SandboxStatus::Active
        && sandbox.runtime.as_ref().is_some_and(|runtime| {
            !runtime.runtime_id.trim().is_empty()
                && !runtime.runtime_version.trim().is_empty()
                && seconds_from_rfc3339(&runtime.bound_at).is_some()
        })
}

/// The fixed owner-facing reason for a pause under a platform spend stop.
pub const PLATFORM_SPEND_PAUSE_NOTICE: &str = "Platform spend limit reached. Your work is saved; the Sandbox will be available again after the limit is raised.";

/// Compose the one notice stored for an inactivity pause.
pub fn inactivity_pause_notice(pause_after_seconds: u64) -> String {
    let minutes = pause_after_seconds / 60;
    format!(
        "Your sandbox has been idle for {minutes} minutes and was paused. Send a message to continue."
    )
}

fn complete_conversation_identity(activity: &SandboxAgentActivity) -> bool {
    matches!(
        (
            activity.conversation_id.as_deref(),
            activity.runtime_generation,
        ),
        (Some(conversation_id), Some(generation))
            if !conversation_id.trim().is_empty()
                && conversation_id.len() <= 512
                && generation > 0
                && activity.last_seq > 0
    )
}
