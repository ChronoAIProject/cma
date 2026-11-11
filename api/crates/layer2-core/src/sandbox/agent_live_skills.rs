//! Durable skill installation history; profile material remains immutable.
use super::{
    Agent, AgentLogKind, AgentOperationError, AgentOperationService, AgentStatus, AgentWorkState,
    AgentWorkStep, ResolvedAgentSkill, SandboxModelError,
};
use crate::profiles::SkillRef;
use crate::{now_rfc3339, AgentId, Caller};
use serde::{Deserialize, Serialize};

const MAX_SKILLS: usize = 32;
const MAX_ATTEMPTS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillInstallStatus {
    Pending,
    Installed,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSkillInstallation {
    pub reference: SkillRef,
    /// Exact effective installation being replaced; absent on legacy additive work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<ResolvedAgentSkill>,
    pub status: AgentSkillInstallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<ResolvedAgentSkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl AgentSkillInstallation {
    pub fn new(reference: SkillRef) -> Self {
        Self {
            reference,
            previous: None,
            status: AgentSkillInstallStatus::Pending,
            material: None,
            error: None,
        }
    }
}
fn valid_reference(reference: &SkillRef) -> bool {
    let id = reference.id.as_str();
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !matches!(id, "nyxid" | "ornn-agent-manual-cli")
        && reference.version.as_ref().is_some_and(|v| {
            !v.is_empty()
                && !v.starts_with('@')
                && v.len() <= 256
                && v.trim() == v
                && !v.chars().any(char::is_control)
        })
}
pub(super) fn validate_installations(
    work: &AgentWorkState,
    profile: &[SkillRef],
) -> Result<(), SandboxModelError> {
    let valid = skill_count(work, profile) <= MAX_SKILLS
        && work.live_skills.len() <= MAX_ATTEMPTS
        && work.live_skills.iter().enumerate().all(|(index, item)| {
            let previous = effective_before(work, &item.reference.id, index);
            valid_reference(&item.reference)
                && match &item.previous {
                    Some(expected) => {
                        previous == Some(expected)
                            && expected.validate().is_ok()
                            && item.reference.source == expected.source
                            && item.reference.version.as_deref() != Some(expected.version.as_str())
                    }
                    None => {
                        !profile.iter().any(|r| r.id == item.reference.id)
                            && !work.live_skills[..index]
                                .iter()
                                .any(|r| r.reference.id == item.reference.id)
                    }
                }
                && item.material.as_ref().is_none_or(|m| {
                    m.validate().is_ok()
                        && m.id == item.reference.id
                        && m.source == item.reference.source
                        && item.reference.version.as_deref() == Some(m.version.as_str())
                })
                && match item.status {
                    AgentSkillInstallStatus::Pending => {
                        index + 1 == work.live_skills.len() && item.error.is_none()
                    }
                    AgentSkillInstallStatus::Installed => {
                        item.material.is_some() && item.error.is_none()
                    }
                    AgentSkillInstallStatus::Failed => item
                        .error
                        .as_ref()
                        .is_some_and(|e| !e.is_empty() && e.len() <= 256),
                }
        })
        && (work.step != AgentWorkStep::InstallSkill
            || work
                .live_skills
                .last()
                .is_some_and(|i| i.status == AgentSkillInstallStatus::Pending));
    if valid {
        Ok(())
    } else {
        Err(SandboxModelError::invalid_value("agent.work.live_skills"))
    }
}
/// Latest runtime-confirmed material; failed attempts never replace it.
pub fn effective<'a>(
    work: &'a AgentWorkState,
    id: &crate::SkillId,
) -> Option<&'a ResolvedAgentSkill> {
    effective_before(work, id, work.live_skills.len())
}
fn effective_before<'a>(
    work: &'a AgentWorkState,
    id: &crate::SkillId,
    end: usize,
) -> Option<&'a ResolvedAgentSkill> {
    work.live_skills[..end]
        .iter()
        .rev()
        .find(|item| item.reference.id == *id && item.status == AgentSkillInstallStatus::Installed)
        .and_then(|item| item.material.as_ref())
        .or_else(|| work.resolved_skills.as_ref()?.iter().find(|m| m.id == *id))
}
fn skill_count(work: &AgentWorkState, profile: &[SkillRef]) -> usize {
    profile
        .iter()
        .map(|r| &r.id)
        .chain(work.live_skills.iter().map(|r| &r.reference.id))
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}
/// Capacity for a new identifier and for replacing an installed identifier.
/// Activity, ownership and observed runtime support are checked separately.
pub fn capacity(agent: &Agent) -> (bool, bool) {
    if agent.work.live_skills.len() >= MAX_ATTEMPTS {
        return (false, false);
    }
    let installed = agent
        .work
        .live_skills
        .iter()
        .filter(|item| item.status == AgentSkillInstallStatus::Installed)
        .filter_map(|item| item.material.as_ref())
        .chain(agent.work.resolved_skills.iter().flatten());
    let upgrade = installed.into_iter().any(|material| {
        valid_reference(&SkillRef {
            source: material.source,
            id: material.id.clone(),
            version: Some(material.version.clone()),
        })
    });
    (
        skill_count(&agent.work, &agent.profile.content.skills) < MAX_SKILLS,
        upgrade,
    )
}

fn intent(agent: &Agent, reference: SkillRef) -> AgentSkillInstallation {
    let previous = effective(&agent.work, &reference.id).cloned();
    AgentSkillInstallation {
        previous,
        ..AgentSkillInstallation::new(reference)
    }
}

pub fn idle(agent: &Agent) -> bool {
    agent.status == AgentStatus::Ready
        && agent.work.step == AgentWorkStep::None
        && !agent
            .work
            .live_skills
            .iter()
            .any(|s| s.status == AgentSkillInstallStatus::Pending)
        && agent.requested_stop_reason.is_none()
        && agent.conversation.active_turn.is_none()
        && agent
            .conversation
            .queued_messages
            .iter()
            .all(|m| m.state == super::QueuedMessageState::Delivered)
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_none_or(|m| m.is_terminal())
        && agent
            .conversation
            .operation
            .as_ref()
            .is_none_or(|o| !o.blocks_dispatch())
}

/// Exact exception used by both durable conversation stores. Everything in work
/// other than scheduling this one request must retain its previous value.
pub fn is_install_admission(current: &Agent, next: &Agent) -> bool {
    let Some(item) = next.work.live_skills.last() else {
        return false;
    };
    if !idle(current)
        || !valid_reference(&item.reference)
        || *item != intent(current, item.reference.clone())
        || next.work.live_skills.len() != current.work.live_skills.len() + 1
        || !next.work.live_skills.starts_with(&current.work.live_skills)
        || validate_installations(&next.work, &next.profile.content.skills).is_err()
    {
        return false;
    }
    let mut expected = current.work.clone();
    expected.live_skills.push(item.clone());
    expected.schedule(AgentWorkStep::InstallSkill, next.updated_at.clone());
    let mut conversation = next.conversation.clone();
    conversation.last_seq = current.conversation.last_seq;
    expected == next.work && current.status == next.status && conversation == current.conversation
}

impl AgentOperationService {
    pub async fn install_skill(
        &self,
        caller: &Caller,
        id: &AgentId,
        reference: SkillRef,
    ) -> Result<Agent, AgentOperationError> {
        self.require_owner_authority(caller)?;
        if !valid_reference(&reference) {
            return Err(AgentOperationError::InvalidRequest(
                "choose a valid skill and exact version".into(),
            ));
        }
        for _ in 0..super::agent_operation_service::MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            if let Some(existing) = current.work.live_skills.last().filter(|item| {
                item.status == AgentSkillInstallStatus::Pending && item.reference == reference
            }) {
                let _ = existing;
                return Ok(current);
            }
            if !idle(&current) {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            let previous = effective(&current.work, &reference.id);
            if previous.is_some_and(|m| {
                m.source == reference.source && Some(&m.version) == reference.version.as_ref()
            }) {
                return Ok(current);
            }
            if previous.is_none() {
                // Failed additive work retains its original per-skill identity.
                // It cannot be silently reinterpreted as a fresh installation.
                if let Some(existing) = current
                    .work
                    .live_skills
                    .iter()
                    .find(|i| i.reference.id == reference.id)
                {
                    return if existing.reference == reference {
                        Ok(current)
                    } else {
                        Err(AgentOperationError::InvalidRequest(
                            "previous skill installation failed; no confirmed version to upgrade"
                                .into(),
                        ))
                    };
                }
                if current
                    .profile
                    .content
                    .skills
                    .iter()
                    .any(|s| s.id == reference.id)
                {
                    return Err(AgentOperationError::InvalidRequest(
                        "previous skill material is unavailable".into(),
                    ));
                }
                if skill_count(&current.work, &current.profile.content.skills) >= MAX_SKILLS {
                    return Err(AgentOperationError::InvalidRequest(
                        "skill limit reached".into(),
                    ));
                }
            }
            if current.work.live_skills.len() >= MAX_ATTEMPTS {
                return Err(AgentOperationError::InvalidRequest(
                    "skill installation history limit reached".into(),
                ));
            }
            self.require_agent_connections(&current).await?;
            let (parent, queues) = self.parent_admission(&current).await?;
            // A ready child can precede the parent's final active publication.
            // Skill stores only admit an active parent; do not spin on a
            // deterministic store refusal and misreport it as an operation lock.
            if queues
                || !matches!(
                    parent.status,
                    super::SandboxStatus::Active | super::SandboxStatus::CredentialRotating
                )
            {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            let at = now_rfc3339();
            let mut next = current.clone();
            next.work
                .live_skills
                .push(intent(&current, reference.clone()));
            next.work.schedule(AgentWorkStep::InstallSkill, at.clone());
            let logs = vec![AgentLogKind::SkillInstallationChanged {
                skill_id: reference.id.as_str().into(),
                status: AgentSkillInstallStatus::Pending,
            }];
            match self
                .commit_admission(&current, next, logs, at, parent)
                .await
            {
                Ok((agent, _)) => return Ok(agent),
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        // Revision contention does not prove that another operation owns this
        // Agent. Keep the bounded retry, but do not project it as a busy Agent.
        Err(AgentOperationError::Unavailable(
            "skill admission could not commit after concurrent changes; retry the request".into(),
        ))
    }
}

/// The worker may only pin material, retry the same intent, or publish its result.
/// It cannot edit conversation, profile, other skills, or lifecycle receipts.
pub fn validate_install_work(
    current: &Agent,
    worktree: &super::Worktree,
    mutation: &super::store::AgentWorkMutation,
) -> Result<(), super::store::SandboxStoreError> {
    use super::store::SandboxStoreError as Error;
    let next = &mutation.agent;
    if current.status != AgentStatus::Ready
        || current.conversation.active_turn.is_some()
        || current.requested_stop_reason.is_some()
        || worktree.status != super::WorktreeStatus::Ready
        || mutation.worktree.is_some()
        || mutation.sandbox_event.is_some()
    {
        return Err(Error::Conflict);
    }
    let old = current.work.live_skills.last().ok_or(Error::Conflict)?;
    let new = next.work.live_skills.last().ok_or(Error::Conflict)?;
    if old.status != AgentSkillInstallStatus::Pending
        || old.reference != new.reference
        || old.previous != new.previous
        || current.work.live_skills.len() != next.work.live_skills.len()
        || current.work.live_skills[..current.work.live_skills.len() - 1]
            != next.work.live_skills[..next.work.live_skills.len() - 1]
    {
        return Err(Error::Conflict);
    }
    let mut expected = current.clone();
    match new.status {
        AgentSkillInstallStatus::Pending if old.material.is_none() && new.material.is_some() => {
            if new.error.is_some() || !mutation.agent_logs.is_empty() {
                return Err(Error::Conflict);
            }
            expected
                .work
                .live_skills
                .last_mut()
                .expect("present")
                .material = new.material.clone();
        }
        AgentSkillInstallStatus::Pending if old == new => {
            if (!mutation.agent_logs.is_empty()
                && !super::store::agent_cleanup_capacity_retry(current, mutation))
                || next.work.ready_at <= mutation.appended_at
            {
                return Err(Error::Conflict);
            }
            expected
                .work
                .retry(&mutation.holder, mutation.fence, next.work.ready_at.clone())
                .map_err(|_| Error::Conflict)?;
        }
        AgentSkillInstallStatus::Installed | AgentSkillInstallStatus::Failed => {
            if old.material != new.material
                || (new.status == AgentSkillInstallStatus::Installed && old.material.is_none())
                || mutation.agent_logs
                    != vec![AgentLogKind::SkillInstallationChanged {
                        skill_id: new.reference.id.as_str().into(),
                        status: new.status,
                    }]
            {
                return Err(Error::Conflict);
            }
            *expected.work.live_skills.last_mut().expect("present") = new.clone();
            expected
                .work
                .schedule(AgentWorkStep::None, mutation.appended_at.clone());
        }
        _ => return Err(Error::Conflict),
    }
    expected.updated_at = mutation.appended_at.clone();
    expected.revision = next.revision;
    expected.conversation.last_seq = next.conversation.last_seq;
    if expected == *next {
        Ok(())
    } else {
        Err(Error::Conflict)
    }
}
