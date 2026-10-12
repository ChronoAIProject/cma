use serde::{Deserialize, Serialize};

use crate::profiles::{
    AgentProfileRef, PermissionLevel, SandboxProfileRef, SkillRef, ThinkingEffort,
    WorkspaceProfileRef,
};
use crate::ServiceId;

use super::TriggerError;

pub const MAX_FIRST_MESSAGE_BYTES: usize = 32 * 1_024;
pub const MAX_QUICK_MESSAGES: usize = 8;
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
pub struct TriggerImage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The immutable OCI reference supplied by Image Manager when a catalog image
    /// is selected. Keeping this alongside the catalog id pins the exact runtime
    /// image without making an Image Manager credential part of a durable Trigger
    /// revision. The field is absent only when the system default image is used
    /// (that case has no `TriggerImage` object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl TriggerImage {
    fn validate(&self) -> Result<(), TriggerError> {
        if self.id.trim().is_empty() || self.id.len() > 256 {
            return Err(TriggerError::new("invalid_image", "target.inline.image.id"));
        }
        if self
            .version
            .as_ref()
            .is_some_and(|version| version.is_empty() || version.len() > 256)
        {
            return Err(TriggerError::new(
                "invalid_image_version",
                "target.inline.image.version",
            ));
        }
        let Some(reference) = &self.reference else {
            return Err(TriggerError::new(
                "invalid_image_reference",
                "target.inline.image.reference",
            ));
        };
        if !valid_immutable_image(reference) {
            return Err(TriggerError::new(
                "invalid_image_reference",
                "target.inline.image.reference",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRepository {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl TriggerRepository {
    fn validate(&self) -> Result<(), TriggerError> {
        let parsed = url::Url::parse(&self.url)
            .map_err(|_| TriggerError::new("invalid_repository", "target.inline.workspace"))?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("github.com")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed
                .path_segments()
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .count()
                != 2
        {
            return Err(TriggerError::new(
                "invalid_repository",
                "target.inline.workspace",
            ));
        }
        if self.reference.as_ref().is_some_and(|reference| {
            reference.is_empty() || reference.len() > 255 || reference.chars().any(char::is_control)
        }) {
            return Err(TriggerError::new(
                "invalid_repository_reference",
                "target.inline.workspace.reference",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerInlineWorkspace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<TriggerRepository>,
}

impl TriggerInlineWorkspace {
    fn validate(&self) -> Result<(), TriggerError> {
        if let Some(repository) = &self.repository {
            repository.validate()?
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerInlineAgent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub reasoning_effort: ThinkingEffort,
    pub permission_mode: PermissionLevel,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub services: Vec<ServiceId>,
}

impl TriggerInlineAgent {
    fn validate(&self) -> Result<(), TriggerError> {
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err(TriggerError::new(
                "invalid_model",
                "target.inline.agent.model",
            ));
        }
        if self.skills.len() > 32 {
            return Err(TriggerError::new(
                "too_many_skills",
                "target.inline.agent.skills",
            ));
        }
        if self.system_prompt.len() > 32 * 1_024 {
            return Err(TriggerError::new(
                "system_prompt_too_long",
                "target.inline.agent.system_prompt",
            ));
        }
        if self.skills.iter().any(|skill| skill.id.as_str().is_empty()) {
            return Err(TriggerError::new(
                "invalid_skill",
                "target.inline.agent.skills",
            ));
        }
        if self.services.len() > 64
            || self
                .services
                .iter()
                .any(|service| service.as_str().is_empty())
        {
            return Err(TriggerError::new(
                "invalid_service",
                "target.inline.agent.services",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerInlineTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<TriggerImage>,
    pub agent: TriggerInlineAgent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<TriggerInlineWorkspace>,
}

impl TriggerInlineTarget {
    /// Validates the inline authoring payload before an adapter materializes it.
    pub fn validate(&self) -> Result<(), TriggerError> {
        if let Some(image) = &self.image {
            image.validate()?
        }
        self.agent.validate()?;
        if let Some(workspace) = &self.workspace {
            workspace.validate()?
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerTarget {
    #[serde(
        default = "empty_sandbox_profile",
        skip_serializing_if = "is_empty_sandbox_profile"
    )]
    pub sandbox_profile: SandboxProfileRef,
    #[serde(
        default = "empty_workspace_profile",
        skip_serializing_if = "is_empty_workspace_profile"
    )]
    pub workspace_profile: WorkspaceProfileRef,
    #[serde(
        default = "empty_agent_profile",
        skip_serializing_if = "is_empty_agent_profile"
    )]
    pub agent_profile: AgentProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<TriggerInlineTarget>,
}

impl TriggerTarget {
    fn validate(&self) -> Result<(), TriggerError> {
        if let Some(inline) = &self.inline {
            inline.validate()?;
            return Ok(());
        }
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

fn empty_sandbox_profile() -> SandboxProfileRef {
    SandboxProfileRef::new(crate::SandboxProfileId(String::new()), 0)
}

fn empty_workspace_profile() -> WorkspaceProfileRef {
    WorkspaceProfileRef::new(crate::WorkspaceProfileId(String::new()), 0)
}

fn empty_agent_profile() -> AgentProfileRef {
    AgentProfileRef::new(crate::AgentProfileId(String::new()), 0)
}

fn is_empty_sandbox_profile(value: &SandboxProfileRef) -> bool {
    value.id.as_str().is_empty() && value.revision == 0
}

fn is_empty_workspace_profile(value: &WorkspaceProfileRef) -> bool {
    value.id.as_str().is_empty() && value.revision == 0
}

fn is_empty_agent_profile(value: &AgentProfileRef) -> bool {
    value.id.as_str().is_empty() && value.revision == 0
}

fn valid_immutable_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 2_048
        && !name.contains('@')
        && !name.contains("://")
        && !name.starts_with(['.', '-', '_', '/', ':'])
        && !name.ends_with(['.', '-', '_', '/', ':'])
        && !name.contains("//")
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quick_messages: Vec<TriggerQuickMessage>,
    pub consent: TriggerConsent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerQuickMessage {
    pub name: String,
    pub description: String,
    pub text: String,
}

impl TriggerQuickMessage {
    fn validate(&self) -> Result<(), TriggerError> {
        if self.name.trim().is_empty()
            || self.name.len() > MAX_TITLE_BYTES
            || self.name.chars().any(char::is_control)
        {
            return Err(TriggerError::new(
                "invalid_quick_message_name",
                "quick_messages",
            ));
        }
        if self.description.trim().is_empty() || self.description.len() > MAX_DESCRIPTION_BYTES {
            return Err(TriggerError::new(
                "invalid_quick_message_description",
                "quick_messages",
            ));
        }
        if self.text.trim().is_empty() || self.text.len() > MAX_FIRST_MESSAGE_BYTES {
            return Err(TriggerError::new(
                "invalid_quick_message_text",
                "quick_messages",
            ));
        }
        Ok(())
    }
}

impl TriggerContent {
    pub fn validate(&self) -> Result<(), TriggerError> {
        self.presentation.validate()?;
        self.target.validate()?;
        if self.first_message.len() > MAX_FIRST_MESSAGE_BYTES {
            return Err(TriggerError::new("first_message_too_long", "first_message"));
        }
        if self.quick_messages.len() > MAX_QUICK_MESSAGES {
            return Err(TriggerError::new(
                "too_many_quick_messages",
                "quick_messages",
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for message in &self.quick_messages {
            message.validate()?;
            if !names.insert(message.name.trim().to_lowercase()) {
                return Err(TriggerError::new(
                    "duplicate_quick_message_name",
                    "quick_messages",
                ));
            }
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

#[cfg(test)]
mod quick_message_tests {
    use super::*;
    use crate::trigger_domain::{trigger_content_digest, TriggerRevision};

    fn legacy() -> serde_json::Value {
        serde_json::json!({
            "presentation": {"title":"Example", "description":"", "button_label":"run_with_cma", "theme":"auto"},
            "target": {"sandbox_profile":{"id":"sbp_example","revision":1}, "workspace_profile":{"id":"wkp_example","revision":1}, "agent_profile":{"id":"agp_example","revision":1}},
            "reuse":{"sandbox":"reuse_existing","workspace":"reuse_existing","agent":"resume_existing"},
            "first_message":"Hello", "consent":{"always_review":false}
        })
    }

    fn message() -> TriggerQuickMessage {
        TriggerQuickMessage {
            name: "Introduce the project".into(),
            description: "Describe the project and its uses.".into(),
            text: "Read the repository introduction and describe it.".into(),
        }
    }

    #[test]
    fn empty_shortcuts_preserve_legacy_serialization_and_revision_digests() {
        let original = legacy();
        let content: TriggerContent = serde_json::from_value(original.clone()).unwrap();
        assert!(content.quick_messages.is_empty());
        assert_eq!(serde_json::to_value(&content).unwrap(), original);
        let mut explicit = original.clone();
        explicit["quick_messages"] = serde_json::json!([]);
        let explicit: TriggerContent = serde_json::from_value(explicit).unwrap();
        assert_eq!(
            trigger_content_digest(&content).unwrap(),
            trigger_content_digest(&explicit).unwrap()
        );
        let revision = TriggerRevision::new(1, content, "2026-09-09T00:00:00Z".into()).unwrap();
        revision.validate().unwrap();
        let mut changed = revision.clone();
        changed.content.quick_messages.push(message());
        assert_eq!(
            changed.validate().unwrap_err().code,
            "trigger_digest_mismatch"
        );
    }

    #[test]
    fn shortcuts_require_bounded_unique_names_descriptions_and_text() {
        let mut content: TriggerContent = serde_json::from_value(legacy()).unwrap();
        content.quick_messages.push(message());
        content.validate().unwrap();
        for (field, invalid) in [
            ("name", " ".into()),
            ("name", "x".repeat(121)),
            ("name", "line\nbreak".into()),
            ("description", " ".into()),
            ("description", "x".repeat(2001)),
            ("text", " ".into()),
            ("text", "x".repeat(32769)),
        ] {
            let mut value = serde_json::to_value(&content).unwrap();
            value["quick_messages"][0][field] = serde_json::Value::String(invalid);
            assert!(
                serde_json::from_value::<TriggerContent>(value)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{field}"
            );
        }
        let mut duplicate = message();
        duplicate.name = " INTRODUCE THE PROJECT ".into();
        content.quick_messages.push(duplicate);
        assert_eq!(
            content.validate().unwrap_err().code,
            "duplicate_quick_message_name"
        );
        content.quick_messages = (0..9)
            .map(|index| TriggerQuickMessage {
                name: index.to_string(),
                ..message()
            })
            .collect();
        assert_eq!(
            content.validate().unwrap_err().code,
            "too_many_quick_messages"
        );
    }
}

#[cfg(test)]
mod inline_target_tests {
    use super::*;

    fn inline_content() -> TriggerContent {
        serde_json::from_value(serde_json::json!({
            "presentation": {
                "title": "Inline Trigger",
                "description": "",
                "button_label": "try_it",
                "theme": "dark"
            },
            "target": {
                "inline": {
                    "image": {
                        "id": "private-codex",
                        "version": "v3",
                        "reference": "registry.example/codex@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    },
                    "agent": {
                        "model": "gpt-5-codex",
                        "reasoning_effort": "high",
                        "permission_mode": "guarded",
                        "skills": [{"source": "ornn", "id": "repo-search"}],
                        "system_prompt": "Keep changes small.",
                        "services": ["api-github"]
                    },
                    "workspace": {
                        "repository": {
                            "url": "https://github.com/chronoai/project",
                            "reference": "main"
                        }
                    }
                }
            },
            "reuse": {"sandbox": "always_new", "workspace": "always_new", "agent": "always_new"},
            "first_message": "Inspect the repository",
            "consent": {"always_review": true}
        }))
        .expect("inline trigger JSON should decode")
    }

    #[test]
    fn inline_target_validates_without_profile_references() {
        inline_content()
            .validate()
            .expect("inline target should validate without legacy profiles");
    }

    #[test]
    fn inline_target_rejects_mutable_image_reference() {
        let mut content = inline_content();
        content
            .target
            .inline
            .as_mut()
            .expect("inline target")
            .image
            .as_mut()
            .expect("image")
            .reference = Some("registry.example/codex:latest".into());
        let error = content
            .validate()
            .expect_err("mutable image must be rejected");
        assert_eq!(error.code, "invalid_image_reference");
        assert_eq!(error.field, "target.inline.image.reference");
    }
}
