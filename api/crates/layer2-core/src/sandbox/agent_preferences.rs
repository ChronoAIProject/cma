//! Mutable owner presentation and instruction preferences. Creation remains immutable.
use serde::{Deserialize, Serialize};

use super::{Agent, AgentLogKind, AgentOperationError, AgentOperationService, SandboxModelError};
use crate::{now_rfc3339, AgentId, Caller};

/// Product-owned managed guidance can be kept separate from the editable owner
/// prompt without teaching the conversation adapter product-specific text.
pub trait AgentInstructionPolicy: Send + Sync {
    fn owner_prompt(&self, agent: &Agent) -> String;
    fn compose(&self, agent: &Agent, owner_prompt: &str) -> String;
    /// Instructions already pinned for this Agent supersede the deployment's
    /// default whenever maintenance refreshes its managed context.
    fn pinned_managed_guidance(&self, _agent: &Agent) -> Option<&'static str> {
        None
    }
}

pub(super) struct UnmanagedInstructions;
impl AgentInstructionPolicy for UnmanagedInstructions {
    fn owner_prompt(&self, agent: &Agent) -> String {
        agent.system_prompt().to_owned()
    }
    fn compose(&self, _agent: &Agent, owner_prompt: &str) -> String {
        owner_prompt.to_owned()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPreferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub revision: u64,
}

impl AgentPreferences {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if self
            .title
            .as_deref()
            .is_some_and(|title| !valid_title(title))
        {
            return Err(SandboxModelError::invalid_value("agent.preferences.title"));
        }
        Ok(())
    }
}

fn valid_title(title: &str) -> bool {
    !title.is_empty()
        && title.trim() == title
        && title.chars().count() <= 64
        && !title.chars().any(char::is_control)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPreferencesUpdate {
    Title {
        expected_revision: u64,
        title: String,
    },
    SystemPrompt {
        expected_revision: u64,
        system_prompt: String,
    },
}
impl AgentPreferencesUpdate {
    pub fn expected_revision(&self) -> u64 {
        match self {
            Self::Title {
                expected_revision, ..
            }
            | Self::SystemPrompt {
                expected_revision, ..
            } => *expected_revision,
        }
    }
    pub fn validate(&self) -> Result<(), AgentOperationError> {
        let valid = match self {
            Self::Title { title, .. } => valid_title(title),
            Self::SystemPrompt { system_prompt, .. } => {
                system_prompt.len() <= 32 * 1024 && !system_prompt.contains('\0')
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AgentOperationError::InvalidRequest(
                "invalid Agent preference".into(),
            ))
        }
    }
}

impl Agent {
    pub fn system_prompt(&self) -> &str {
        self.conversation
            .settings
            .as_ref()
            .and_then(|settings| settings.system_prompt.as_deref())
            .unwrap_or(&self.profile.content.system_prompt)
    }
}

impl AgentOperationService {
    pub fn with_instruction_policy(
        &self,
        policy: std::sync::Arc<dyn AgentInstructionPolicy>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            skill_upgrades_enabled: self.skill_upgrades_enabled,
            model_credit_required: self.model_credit_required,
            model_funding: self.model_funding.clone(),
            funded_message: self.funded_message.clone(),
            instruction_policy: policy,
            settings_authority: self.settings_authority.clone(),
            command_claim: self.command_claim.clone(),
            execution_authority: self.execution_authority.clone(),
            trigger_authority: self.trigger_authority.clone(),
            connections: self.connections.clone(),
            store: self.store.clone(),
            runtimes: self.runtimes.clone(),
        })
    }
    pub fn owner_system_prompt(&self, agent: &Agent) -> String {
        self.instruction_policy.owner_prompt(agent)
    }

    pub async fn system_prompt_supported(&self, agent: &Agent) -> bool {
        let Ok(target) = super::agent_operation_dispatch::runtime_target(agent) else {
            return false;
        };
        match self.runtimes.resolve(&target).await {
            Ok(runtime) => runtime.input.system_prompt_supported().await,
            Err(_) => false,
        }
    }

    pub(super) fn validate_preferences_revision(
        &self,
        current: &Agent,
    ) -> Result<(), AgentOperationError> {
        if self
            .settings_authority
            .as_ref()
            .and_then(|authority| authority.preferences())
            .is_some_and(|update| {
                update.expected_revision() != current.conversation.preferences.revision
            })
        {
            return Err(AgentOperationError::PreferencesRevisionMismatch);
        }
        Ok(())
    }

    pub async fn update_preferences(
        &self,
        caller: &Caller,
        id: &AgentId,
        update: AgentPreferencesUpdate,
    ) -> Result<Agent, AgentOperationError> {
        self.require_owner_authority(caller)?;
        update.validate()?;
        for _ in 0..super::agent_operation_service::MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            if current.conversation.preferences.revision != update.expected_revision() {
                return Err(AgentOperationError::PreferencesRevisionMismatch);
            }
            match &update {
                AgentPreferencesUpdate::SystemPrompt { system_prompt, .. } => {
                    let mut settings = current.settings();
                    settings.system_prompt =
                        Some(self.instruction_policy.compose(&current, system_prompt));
                    // The private expected preference revision is rechecked on every
                    // admission retry, including when no resource sharing is composed.
                    return self
                        .update_settings_with_revision(
                            caller,
                            id,
                            settings,
                            update.expected_revision(),
                        )
                        .await;
                }
                AgentPreferencesUpdate::Title { title, .. } => {
                    if current.conversation.preferences.title.as_ref() == Some(title) {
                        return Ok(current);
                    }
                    if !current.status.is_live()
                        || current.conversation.runtime_generation.is_none()
                    {
                        return Err(AgentOperationError::StatusConflict {
                            status: current.status,
                        });
                    }
                    let (parent, _) = self.parent_admission(&current).await?;
                    let mut next = current.clone();
                    next.conversation.preferences.title = Some(title.clone());
                    next.conversation.preferences.revision =
                        update.expected_revision().checked_add(1).ok_or_else(|| {
                            AgentOperationError::Corrupt("preferences revision overflow".into())
                        })?;
                    let logs = vec![AgentLogKind::TitleSet {
                        title: title.clone(),
                        source: "owner_override".into(),
                    }];
                    match self
                        .commit_admission(&current, next, logs, now_rfc3339(), parent)
                        .await
                    {
                        Ok((agent, _)) => return Ok(agent),
                        Err(AgentOperationError::Conflict) => continue,
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        Err(AgentOperationError::ConfigurationAdmissionConflict)
    }
}

pub(super) fn validate_preferences_mutation(
    current: &Agent,
    mutation: &super::store::AgentConversationMutation,
) -> Result<(), super::store::SandboxStoreError> {
    use super::store::SandboxStoreError;
    let next = &mutation.agent;
    let before = &current.conversation.preferences;
    let after = &next.conversation.preferences;
    if before == after {
        return Ok(());
    }
    if before.revision.checked_add(1) != Some(after.revision) || after.validate().is_err() {
        return Err(SandboxStoreError::Conflict);
    }
    let title_change = before.title != after.title;
    if title_change {
        let mut permitted = current.conversation.clone();
        permitted.preferences = after.clone();
        permitted.last_seq = next.conversation.last_seq;
        if permitted != next.conversation
            || current.status != next.status
            || mutation.admission_guard.is_none()
            || mutation.agent_logs
                != vec![AgentLogKind::TitleSet {
                    title: after.title.clone().ok_or(SandboxStoreError::Conflict)?,
                    source: "owner_override".into(),
                }]
        {
            return Err(SandboxStoreError::Conflict);
        }
    } else if current.settings().system_prompt == next.settings().system_prompt
        || mutation.admission_guard.is_none()
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}
