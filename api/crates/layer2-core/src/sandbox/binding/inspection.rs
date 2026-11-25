use super::{BindingConversation, BindingError, BindingService, EXISTING_AGENT_CONVERSATION};
use crate::profiles::AgentProfileContent;
use crate::sandbox::{Agent, AgentSkillInstallStatus, AgentStatus};
use crate::Caller;
use serde::{Deserialize, Serialize};

pub struct BindingConversationInspection {
    pub conversation: BindingConversation,
    pub incarnation: u64,
    pub transport_conversation_id: String,
    pub agent_status: Option<AgentStatus>,
    pub profile_drift: Option<BindingProfileDrift>,
}

/// Content-free comparison of an owned partition Agent with its assigned profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingProfileDrift {
    pub assigned_profile_digest: String,
    pub prompt: bool,
    pub skills: bool,
    pub model: bool,
}

pub(super) fn profile_drift(
    agent: &Agent,
    expected: &AgentProfileContent,
    digest: &str,
) -> BindingProfileDrift {
    let mut skills = agent.profile.content.skills.clone();
    for installation in &agent.work.live_skills {
        if installation.status != AgentSkillInstallStatus::Installed {
            continue;
        }
        let reference = &installation.reference;
        if let Some(existing) = skills.iter_mut().find(|old| old.id == reference.id) {
            *existing = reference.clone();
        } else {
            skills.push(reference.clone());
        }
    }
    let skills_match = skills.len() == expected.skills.len()
        && expected.skills.iter().all(|wanted| {
            skills.iter().any(|actual| {
                actual.id == wanted.id
                    && actual.source == wanted.source
                    && actual.version == wanted.version
            })
        });
    BindingProfileDrift {
        assigned_profile_digest: digest.to_owned(),
        prompt: agent.system_prompt() != expected.system_prompt,
        skills: !skills_match,
        model: expected
            .model
            .as_ref()
            .is_some_and(|model| agent.selected_model().as_ref() != Some(model)),
    }
}

impl BindingService {
    pub async fn inspect_conversation(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
    ) -> Result<BindingConversationInspection, BindingError> {
        if !super::model::valid_id(conversation_id, 8) {
            return Err(BindingError::InvalidRequest(
                "invalid conversation identifier".into(),
            ));
        }
        let binding = self.read_for_retirement(caller, id).await?;
        let conversation_id = if binding.existing_agent.is_some() {
            EXISTING_AGENT_CONVERSATION
        } else {
            conversation_id
        };
        let transport_conversation_id = conversation_id.to_owned();
        let resets = self.store.conversation_reset_state(id).await?;
        let head = resets.head(binding.generation, conversation_id);
        let incarnation = head.map_or(0, |h| h.incarnation);
        let conversation_id =
            head.map_or(conversation_id, |h| h.successor.conversation_id.as_str());
        let conversation = self
            .store
            .binding_conversation(id, binding.generation, conversation_id)
            .await?
            .or_else(|| head.map(|h| h.successor.clone()))
            .ok_or(BindingError::NotFound)?;
        conversation.validate().map_err(BindingError::Corrupt)?;
        if conversation.binding_id != binding.binding_id
            || conversation.generation != binding.generation
            || conversation.owner != binding.owner
            || conversation.sandbox_id != binding.sandbox_id
            || conversation.workspace_id != binding.workspace_id
        {
            return Err(BindingError::Corrupt(
                "conversation ownership is inconsistent".into(),
            ));
        }
        let agent = self.store.get_agent(&conversation.agent_id).await?;
        if agent.as_ref().is_some_and(|agent| {
            agent.owner != conversation.owner
                || agent.sandbox_id != conversation.sandbox_id
                || agent.workspace_id != conversation.workspace_id
                || agent.worktree_id != conversation.worktree_id
        }) {
            return Err(BindingError::Corrupt(
                "conversation Agent ownership is inconsistent".into(),
            ));
        }
        if self.read_for_retirement(caller, id).await? != binding {
            return Err(BindingError::Conflict);
        }
        let profile_drift = agent
            .as_ref()
            .filter(|_| binding.existing_agent.is_none())
            .map(|agent| profile_drift(agent, &binding.profile.agent, &binding.profile_digest));
        Ok(BindingConversationInspection {
            incarnation,
            transport_conversation_id,
            conversation,
            agent_status: agent.map(|agent| agent.status),
            profile_drift,
        })
    }
}
