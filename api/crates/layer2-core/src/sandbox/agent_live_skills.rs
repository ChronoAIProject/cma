//! Durable additive skills; profile material remains immutable.
use super::{
    Agent, AgentLogKind, AgentOperationError, AgentOperationService, AgentStatus, AgentWorkState,
    AgentWorkStep, ResolvedAgentSkill, SandboxModelError,
};
use crate::profiles::SkillRef;
use crate::{now_rfc3339, AgentId, Caller};
use serde::{Deserialize, Serialize};

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
    let valid = work.live_skills.len() + profile.len() <= 32
        && work.live_skills.iter().enumerate().all(|(index, item)| {
            valid_reference(&item.reference)
                && !profile.iter().any(|r| r.id == item.reference.id)
                && !work.live_skills[..index]
                    .iter()
                    .any(|r| r.reference.id == item.reference.id)
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
        || *item != AgentSkillInstallation::new(item.reference.clone())
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
            if let Some(existing) = current
                .work
                .live_skills
                .iter()
                .find(|i| i.reference.id == reference.id)
            {
                return if existing.reference == reference {
                    Ok(current)
                } else if existing.status == AgentSkillInstallStatus::Pending {
                    Err(AgentOperationError::Conflict)
                } else {
                    Err(AgentOperationError::SkillVersionUpgradeUnsupported {
                        id: reference.id.as_str().into(),
                        installed: existing
                            .material
                            .as_ref()
                            .map(|material| material.version.clone())
                            .or_else(|| existing.reference.version.clone())
                            .unwrap_or_default(),
                        requested: reference.version.clone().unwrap_or_default(),
                    })
                };
            }
            if let Some(existing) = current
                .work
                .resolved_skills
                .as_ref()
                .and_then(|skills| skills.iter().find(|s| s.id == reference.id))
            {
                return if reference.version.as_ref() == Some(&existing.version) {
                    Ok(current)
                } else {
                    Err(AgentOperationError::SkillVersionUpgradeUnsupported {
                        id: reference.id.as_str().into(),
                        installed: existing.version.clone(),
                        requested: reference.version.clone().unwrap_or_default(),
                    })
                };
            }
            if let Some(existing) = current
                .profile
                .content
                .skills
                .iter()
                .find(|skill| skill.id == reference.id)
            {
                return if existing.version == reference.version {
                    Err(AgentOperationError::InvalidRequest(
                        "skill already selected or skill limit reached".into(),
                    ))
                } else {
                    Err(AgentOperationError::SkillVersionUpgradeUnsupported {
                        id: reference.id.as_str().into(),
                        installed: existing.version.clone().unwrap_or_default(),
                        requested: reference.version.clone().unwrap_or_default(),
                    })
                };
            }
            if !idle(&current) {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            if current.work.live_skills.len() + current.profile.content.skills.len() >= 32
                || current
                    .profile
                    .content
                    .skills
                    .iter()
                    .any(|s| s.id == reference.id)
            {
                return Err(AgentOperationError::InvalidRequest(
                    "skill already selected or skill limit reached".into(),
                ));
            }
            self.require_agent_connections(&current).await?;
            let (parent, queues) = self.parent_admission(&current).await?;
            if queues {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            let at = now_rfc3339();
            let mut next = current.clone();
            next.work
                .live_skills
                .push(AgentSkillInstallation::new(reference.clone()));
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
        Err(AgentOperationError::Conflict)
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
            if !mutation.agent_logs.is_empty() || next.work.ready_at <= mutation.appended_at {
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
