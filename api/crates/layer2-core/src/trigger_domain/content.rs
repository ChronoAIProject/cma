use serde::{Deserialize, Serialize};

use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};

use super::TriggerError;

pub const MAX_FIRST_MESSAGE_BYTES: usize = 32 * 1_024;
const MAX_TITLE_BYTES: usize = 120;
const MAX_DESCRIPTION_BYTES: usize = 2_000;
const MAX_SIGNUP_HINT_BYTES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonLabel {
    OpenInCma,
    ChatWithThis,
    RunWithCma,
    TryIt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presentation {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub button_label: ButtonLabel,
    pub theme: Theme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_hint: Option<String>,
}

impl Presentation {
    fn validate(&self) -> Result<(), TriggerError> {
        if self.title.trim().is_empty() || self.title.len() > MAX_TITLE_BYTES {
            return Err(TriggerError::new("invalid_title", "presentation.title"));
        }
        if self.description.len() > MAX_DESCRIPTION_BYTES {
            return Err(TriggerError::new(
                "description_too_long",
                "presentation.description",
            ));
        }
        if self
            .signup_hint
            .as_ref()
            .is_some_and(|hint| hint.len() > MAX_SIGNUP_HINT_BYTES)
        {
            return Err(TriggerError::new(
                "signup_hint_too_long",
                "presentation.signup_hint",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerTarget {
    pub sandbox_profile: SandboxProfileRef,
    pub workspace_profile: WorkspaceProfileRef,
    pub agent_profile: AgentProfileRef,
}

impl TriggerTarget {
    fn validate(&self) -> Result<(), TriggerError> {
        validate_reference(
            self.sandbox_profile.id.as_str(),
            self.sandbox_profile.revision,
            "sbp_",
            "target.sandbox_profile",
        )?;
        validate_reference(
            self.workspace_profile.id.as_str(),
            self.workspace_profile.revision,
            "wkp_",
            "target.workspace_profile",
        )?;
        validate_reference(
            self.agent_profile.id.as_str(),
            self.agent_profile.revision,
            "agp_",
            "target.agent_profile",
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxReusePolicy {
    ReuseExisting,
    AlwaysNew,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceReusePolicy {
    ReuseExisting,
    AlwaysNew,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReusePolicy {
    ResumeExisting,
    AlwaysNew,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReusePolicy {
    pub sandbox: SandboxReusePolicy,
    pub workspace: WorkspaceReusePolicy,
    pub agent: AgentReusePolicy,
}

impl ReusePolicy {
    pub fn what_happens(&self) -> Vec<String> {
        let sandbox = match self.sandbox {
            SandboxReusePolicy::ReuseExisting => {
                "Your newest matching Sandbox is reused when available; otherwise a new Sandbox is created."
            }
            SandboxReusePolicy::AlwaysNew => "A new Sandbox is created for you.",
        };
        let workspace = match self.workspace {
            WorkspaceReusePolicy::ReuseExisting => {
                "A matching Workspace in that Sandbox is reused when available; otherwise a new Workspace is created."
            }
            WorkspaceReusePolicy::AlwaysNew => "A new Workspace is created in that Sandbox.",
        };
        let agent = match self.agent {
            AgentReusePolicy::ResumeExisting => {
                "Your newest live Agent created by this Trigger is resumed when available; otherwise a new Agent is created."
            }
            AgentReusePolicy::AlwaysNew => "A new Agent is created in that Workspace.",
            AgentReusePolicy::Ask => {
                "You choose whether to resume your newest live Agent created by this Trigger or create a new Agent."
            }
        };
        vec![sandbox.into(), workspace.into(), agent.into()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerConsent {
    pub always_review: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerContent {
    pub presentation: Presentation,
    pub target: TriggerTarget,
    pub reuse: ReusePolicy,
    #[serde(default)]
    pub first_message: String,
    pub consent: TriggerConsent,
}

impl TriggerContent {
    pub fn validate(&self) -> Result<(), TriggerError> {
        self.presentation.validate()?;
        self.target.validate()?;
        if self.first_message.len() > MAX_FIRST_MESSAGE_BYTES {
            return Err(TriggerError::new("first_message_too_long", "first_message"));
        }
        Ok(())
    }
}

fn validate_reference(
    id: &str,
    revision: u32,
    prefix: &str,
    field: &'static str,
) -> Result<(), TriggerError> {
    if revision == 0 || !id.starts_with(prefix) || id.len() == prefix.len() {
        Err(TriggerError::new("invalid_profile_reference", field))
    } else {
        Ok(())
    }
}
