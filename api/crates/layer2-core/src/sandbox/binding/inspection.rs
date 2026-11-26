use super::{BindingConversation, BindingError, BindingService, EXISTING_AGENT_CONVERSATION};
use crate::profiles::AgentProfileContent;
use crate::sandbox::{Agent, AgentSkillInstallStatus, AgentStatus};
use crate::Caller;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

/// Content-free inventory used when the Gateway establishes the retained
/// partition baseline. The caller must quiesce legacy unassigned admission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingConversationCatalog {
    pub binding_id: String,
    pub generation: u64,
    pub revision: u64,
    pub transport_conversations: Vec<String>,
}

impl BindingService {
    pub async fn conversation_catalog(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<BindingConversationCatalog, BindingError> {
        const MAX_CONVERSATIONS: usize = 1024;
        let binding = self.read_for_retirement(caller, id).await?;
        if binding.existing_agent.is_some() || binding.previous.is_some() {
            return Err(BindingError::Conflict);
        }
        let resets = self.store.conversation_reset_state(id).await?;
        let mut aliases = std::collections::BTreeMap::new();
        for head in resets
            .heads
            .values()
            .filter(|head| head.successor.generation == binding.generation)
        {
            aliases.insert(
                head.successor.conversation_id.clone(),
                head.transport_conversation_id.clone(),
            );
        }
        for receipt in resets
            .receipts
            .values()
            .filter(|receipt| receipt.head.successor.generation == binding.generation)
        {
            aliases.insert(
                receipt.previous.conversation_id.clone(),
                receipt.head.transport_conversation_id.clone(),
            );
            aliases.insert(
                receipt.head.successor.conversation_id.clone(),
                receipt.head.transport_conversation_id.clone(),
            );
        }
        let mut conversations = BTreeSet::new();
        conversations.extend(aliases.values().cloned());
        let mut after = None;
        let mut count = 0;
        loop {
            let page = self
                .store
                .binding_conversations(id, binding.generation, after.as_deref(), 100)
                .await?;
            if page.len() > 100 {
                return Err(BindingError::Corrupt(
                    "invalid conversation catalog page".into(),
                ));
            }
            if page.is_empty() {
                break;
            }
            for conversation in page {
                conversation.validate().map_err(BindingError::Corrupt)?;
                if conversation.binding_id != binding.binding_id
                    || conversation.owner != binding.owner
                    || conversation.generation != binding.generation
                    || conversation.sandbox_id != binding.sandbox_id
                    || conversation.workspace_id != binding.workspace_id
                    || after
                        .as_ref()
                        .is_some_and(|value: &String| value >= &conversation.conversation_id)
                {
                    return Err(BindingError::Corrupt(
                        "invalid conversation catalog identity".into(),
                    ));
                }
                count += 1;
                if count > MAX_CONVERSATIONS {
                    return Err(BindingError::Conflict);
                }
                conversations.insert(
                    aliases
                        .get(&conversation.conversation_id)
                        .unwrap_or(&conversation.conversation_id)
                        .clone(),
                );
                after = Some(conversation.conversation_id);
            }
        }
        if conversations.len() > MAX_CONVERSATIONS
            || self.read_for_retirement(caller, id).await? != binding
            || self.store.conversation_reset_state(id).await? != resets
        {
            return Err(BindingError::Conflict);
        }
        Ok(BindingConversationCatalog {
            binding_id: binding.binding_id,
            generation: binding.generation,
            revision: binding.revision,
            transport_conversations: conversations.into_iter().collect(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredProfileDrift {
    pub profile_digest: String,
    pub prompt: bool,
    pub skills: bool,
    pub model: bool,
    pub service_scope: bool,
}

impl BindingService {
    pub async fn desired_profile_drift(
        &self,
        caller: &Caller,
        id: &str,
        conversation: &str,
        profile: serde_json::Value,
        digest: &str,
    ) -> Result<Option<DesiredProfileDrift>, BindingError> {
        let binding = self.read_for_retirement(caller, id).await?;
        if binding.existing_agent.is_some() {
            return Err(BindingError::Conflict);
        }
        let mut expected = super::profile::map_profile(
            profile,
            digest,
            self.policy.provider,
            &self.policy.supported_model_api,
            self.policy.bounds,
        )?;
        if expected.owner != binding.owner {
            return Err(BindingError::Forbidden);
        }
        let mapping = self.inspect_conversation(caller, id, conversation).await?;
        let Some(agent) = self.store.get_agent(&mapping.conversation.agent_id).await? else {
            return Ok(None);
        };
        let Some(sandbox) = self
            .store
            .get_sandbox(&mapping.conversation.sandbox_id)
            .await?
        else {
            return Ok(None);
        };
        if agent.owner != binding.owner
            || agent.sandbox_id != sandbox.id
            || sandbox.owner != binding.owner
            || agent.worktree_id != mapping.conversation.worktree_id
            || agent.workspace_id != mapping.conversation.workspace_id
            || self.read_for_retirement(caller, id).await? != binding
        {
            return Err(BindingError::Conflict);
        }
        let Some(environment) = expected.environment.as_mut() else {
            return Ok(None);
        };
        let policy = self
            .policy
            .sandbox
            .current_sandbox_policy()
            .await
            .map_err(super::service::unavailable)?;
        environment.include_baseline_services(policy.baseline_services);
        let drift = profile_drift(&agent, &expected.agent, digest);
        Ok(Some(DesiredProfileDrift {
            profile_digest: digest.to_owned(),
            prompt: drift.prompt,
            skills: drift.skills,
            model: drift.model,
            service_scope: sandbox.credential.effective_reaches()
                != environment.sandbox.nyxid_services,
        }))
    }
}
