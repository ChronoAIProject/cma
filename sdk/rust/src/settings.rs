//! Deployment-owned chat settings exposed through the administration REST API.

use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::{Error, ReasoningEffort, Result, SessionSize};

const DEFAULT_REPOSITORY_URL: &str = "https://github.com/the-omega-institute/trureturing";

/// Repository prepared once for an owner's shared chat host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatWorkspaceSource {
    /// Credential-free HTTPS clone URL.
    pub repository_url: String,
    /// Branch cloned into the main checkout.
    pub branch: String,
}

impl Default for ChatWorkspaceSource {
    fn default() -> Self {
        Self {
            repository_url: DEFAULT_REPOSITORY_URL.into(),
            branch: "dev".into(),
        }
    }
}

/// Agent policy shared by every thread on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatAgentSettings {
    /// Selected model, or the deployment model when absent.
    pub model: Option<String>,
    /// Requested model deliberation.
    pub reasoning_effort: ReasoningEffort,
    /// Deployment-authored agent instructions.
    pub instructions: String,
    /// Whether bounded tool bodies are retained.
    pub record_tool_bodies: bool,
}

impl Default for ChatAgentSettings {
    fn default() -> Self {
        Self {
            model: None,
            reasoning_effort: ReasoningEffort::High,
            instructions: String::new(),
            record_tool_bodies: true,
        }
    }
}

/// Inactivity window before a completely quiet host is paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatInactivity {
    /// Quiet window in whole seconds, from 60 through 86400.
    pub pause_after_seconds: u32,
}

impl Default for ChatInactivity {
    fn default() -> Self {
        Self {
            pause_after_seconds: 900,
        }
    }
}

/// Ordered, idempotent workspace preparation commands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatSetup {
    /// Shell commands run after provision and every revive.
    pub commands: Vec<String>,
}

/// Shared host reservation and thread cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatHostSettings {
    /// Explicit reservation, or the deployment reservation when absent.
    pub size: Option<SessionSize>,
    /// Maximum chat windows one owner may open on the host.
    pub max_threads: u8,
}

impl Default for ChatHostSettings {
    fn default() -> Self {
        Self {
            size: None,
            max_threads: 10,
        }
    }
}

/// Complete replacement value for the optional platform chat setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatSettings {
    /// Master admission switch. Shipped off.
    pub enabled: bool,
    /// Workspace repository and branch.
    pub workspace: ChatWorkspaceSource,
    /// Agent behavior pinned by each host.
    pub agent: ChatAgentSettings,
    /// Ordered texts prepended to every user message at runtime.
    pub message_prefixes: Vec<String>,
    /// Automatic pause policy.
    pub inactivity: ChatInactivity,
    /// Workspace preparation recipe.
    pub setup: ChatSetup,
    /// Shared host resource policy.
    pub host: ChatHostSettings,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            workspace: ChatWorkspaceSource::default(),
            agent: ChatAgentSettings::default(),
            message_prefixes: vec!["/codex-formalize".into()],
            inactivity: ChatInactivity::default(),
            setup: ChatSetup::default(),
            host: ChatHostSettings::default(),
        }
    }
}

impl ChatSettings {
    /// Validate the same bounds enforced by the service before sending a write.
    pub fn validate(&self) -> Result<()> {
        let raw = &self.workspace.repository_url;
        let repository = Url::parse(raw)
            .map_err(|_| Error::protocol("chat repository URL must be a valid HTTPS URL"))?;
        if raw.len() > 512
            || repository.scheme() != "https"
            || repository.host_str().is_none()
            || !repository.username().is_empty()
            || repository.password().is_some()
            || repository.query().is_some()
            || repository.fragment().is_some()
        {
            return Err(Error::protocol(
                "chat repository URL must be credential-free HTTPS and at most 512 bytes",
            ));
        }
        let basename = repository
            .path_segments()
            .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
            .map(|segment| segment.strip_suffix(".git").unwrap_or(segment));
        if !basename.is_some_and(safe_segment) {
            return Err(Error::protocol(
                "chat repository URL must end in a safe repository name",
            ));
        }
        if self.workspace.branch.is_empty()
            || self.workspace.branch.len() > 200
            || self.workspace.branch.chars().any(char::is_whitespace)
        {
            return Err(Error::protocol(
                "chat branch must contain 1 through 200 non-whitespace bytes",
            ));
        }
        if self.message_prefixes.len() > 8
            || self.message_prefixes.iter().any(|prefix| {
                prefix.trim().is_empty() || prefix.len() > 256 || prefix.contains(['\r', '\n'])
            })
        {
            return Err(Error::protocol(
                "chat message prefixes must be at most 8 non-empty single-line entries",
            ));
        }
        if !(60..=86_400).contains(&self.inactivity.pause_after_seconds) {
            return Err(Error::protocol(
                "chat pause interval must be from 60 through 86400 seconds",
            ));
        }
        if self.setup.commands.len() > 16
            || self
                .setup
                .commands
                .iter()
                .any(|command| command.len() > 4_096)
        {
            return Err(Error::protocol(
                "chat setup must contain at most 16 commands of at most 4096 bytes",
            ));
        }
        if !(1..=50).contains(&self.host.max_threads) {
            return Err(Error::protocol("chat thread cap must be from 1 through 50"));
        }
        if self
            .host
            .size
            .is_some_and(|size| size.cpu_millis == 0 || size.memory_mib == 0)
        {
            return Err(Error::protocol(
                "chat host CPU and memory must both be positive",
            ));
        }
        Ok(())
    }
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Chat portion of one revisioned platform settings response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChatSettings {
    /// Revision a conditional write must name.
    pub revision: u64,
    /// Current chat setting; absence means the surface is off.
    pub chat: Option<ChatSettings>,
    /// Previous chat setting, when the most recent previous document had one.
    pub previous_chat: Option<ChatSettings>,
}

#[derive(Deserialize)]
pub(crate) struct SettingsEnvelope {
    revision: u64,
    settings: SettingsSection,
    #[serde(default)]
    previous: Option<SettingsSection>,
}

#[derive(Deserialize)]
struct SettingsSection {
    #[serde(default)]
    chat: Option<ChatSettings>,
}

impl SettingsEnvelope {
    pub(crate) fn into_stored(self) -> Result<StoredChatSettings> {
        if let Some(chat) = &self.settings.chat {
            chat.validate()?;
        }
        let previous_chat = self.previous.and_then(|settings| settings.chat);
        if let Some(chat) = &previous_chat {
            chat.validate()?;
        }
        Ok(StoredChatSettings {
            revision: self.revision,
            chat: self.settings.chat,
            previous_chat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_complete_and_dark() {
        let settings: ChatSettings = serde_json::from_str("{}").expect("defaults deserialize");
        assert_eq!(settings, ChatSettings::default());
        assert!(settings.validate().is_ok());
        assert!(!settings.enabled);
    }

    #[test]
    fn response_decoding_tolerates_additive_server_fields() {
        let envelope: SettingsEnvelope = serde_json::from_value(serde_json::json!({
            "revision": 3,
            "future_envelope": true,
            "settings": {
                "future_platform_setting": true,
                "chat": {
                    "future_chat_setting": true,
                    "workspace": { "future_workspace_setting": true },
                    "agent": { "future_agent_setting": true },
                    "inactivity": { "future_inactivity_setting": true },
                    "setup": { "future_setup_setting": true },
                    "host": { "future_host_setting": true }
                }
            }
        }))
        .expect("additive response fields deserialize");

        let stored = envelope
            .into_stored()
            .expect("defaulted response remains valid");
        assert_eq!(stored.revision, 3);
        assert_eq!(stored.chat, Some(ChatSettings::default()));
    }

    #[test]
    fn every_boundary_is_validated_before_a_request() {
        let invalid = [
            ChatSettings {
                workspace: ChatWorkspaceSource {
                    repository_url: "http://example.test/repo".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ChatSettings {
                workspace: ChatWorkspaceSource {
                    branch: "two words".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ChatSettings {
                workspace: ChatWorkspaceSource {
                    branch: "é".repeat(101),
                    ..Default::default()
                },
                ..Default::default()
            },
            ChatSettings {
                message_prefixes: vec!["prefix".into(); 9],
                ..Default::default()
            },
            ChatSettings {
                message_prefixes: vec!["💡".repeat(65)],
                ..Default::default()
            },
            ChatSettings {
                inactivity: ChatInactivity {
                    pause_after_seconds: 59,
                },
                ..Default::default()
            },
            ChatSettings {
                setup: ChatSetup {
                    commands: vec![String::new(); 17],
                },
                ..Default::default()
            },
            ChatSettings {
                setup: ChatSetup {
                    commands: vec!["💡".repeat(1_025)],
                },
                ..Default::default()
            },
            ChatSettings {
                host: ChatHostSettings {
                    max_threads: 51,
                    ..Default::default()
                },
                ..Default::default()
            },
            ChatSettings {
                host: ChatHostSettings {
                    size: Some(SessionSize {
                        cpu_millis: 0,
                        memory_mib: 512,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        ];
        for settings in invalid {
            assert!(settings.validate().is_err());
        }
    }
}
