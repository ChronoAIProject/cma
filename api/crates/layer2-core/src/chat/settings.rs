//! Validated deployment policy pinned by a chat host when it is created.

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{ReasoningEffort, SessionSize};

const DEFAULT_REPOSITORY_URL: &str = "https://github.com/the-omega-institute/trureturing";
const DEFAULT_BRANCH: &str = "dev";
const DEFAULT_MESSAGE_PREFIX: &str = "$codex-formal-answer";
const DEFAULT_SKILL_SETUP: &str = concat!(
    "set -eu; ",
    "test -f skills/codex-formal-answer/SKILL.md; ",
    "mkdir -p \"$HOME/.agents/skills/codex-formal-answer\" ",
    "\"$HOME/.codex/skills/codex-formal-answer\"; ",
    "cp -R skills/codex-formal-answer/. ",
    "\"$HOME/.agents/skills/codex-formal-answer/\"; ",
    "cp -R skills/codex-formal-answer/. ",
    "\"$HOME/.codex/skills/codex-formal-answer/\""
);

/// The repository prepared once for an owner's shared chat host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatWorkspaceSource {
    pub repository_url: String,
    pub branch: String,
}

impl Default for ChatWorkspaceSource {
    fn default() -> Self {
        Self {
            repository_url: DEFAULT_REPOSITORY_URL.into(),
            branch: DEFAULT_BRANCH.into(),
        }
    }
}

/// Agent choices shared by every thread on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatAgentSettings {
    pub model: Option<String>,
    pub reasoning_effort: ReasoningEffort,
    pub instructions: String,
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

/// When an entirely quiet host is eligible to be paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatInactivity {
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatSetup {
    pub commands: Vec<String>,
}

impl Default for ChatSetup {
    fn default() -> Self {
        Self {
            commands: vec![DEFAULT_SKILL_SETUP.into()],
        }
    }
}

/// Resource bounds for one shared host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatHostSettings {
    pub size: Option<SessionSize>,
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

/// Complete deployment-owned policy for the chat surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatSettings {
    pub enabled: bool,
    pub workspace: ChatWorkspaceSource,
    pub agent: ChatAgentSettings,
    pub message_prefixes: Vec<String>,
    pub inactivity: ChatInactivity,
    pub setup: ChatSetup,
    pub host: ChatHostSettings,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            workspace: ChatWorkspaceSource::default(),
            agent: ChatAgentSettings::default(),
            message_prefixes: vec![DEFAULT_MESSAGE_PREFIX.into()],
            inactivity: ChatInactivity::default(),
            setup: ChatSetup::default(),
            host: ChatHostSettings::default(),
        }
    }
}

/// One safe validation refusal, with no submitted value retained in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSettingsViolation {
    field: &'static str,
    path: String,
    detail: &'static str,
}

impl ChatSettingsViolation {
    fn at(field: &'static str, path: impl Into<String>, detail: &'static str) -> Self {
        Self {
            field,
            path: path.into(),
            detail,
        }
    }

    pub fn field(&self) -> &'static str {
        self.field
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn detail(&self) -> &'static str {
        self.detail
    }
}

impl std::fmt::Display for ChatSettingsViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.path, self.detail)
    }
}

impl std::error::Error for ChatSettingsViolation {}

impl ChatSettings {
    /// Validate every externally configurable bound before the object is stored.
    pub fn validate(&self) -> Result<(), ChatSettingsViolation> {
        self.workspace_directory()?;

        let branch = &self.workspace.branch;
        if branch.is_empty() || branch.len() > 200 || branch.chars().any(char::is_whitespace) {
            return Err(ChatSettingsViolation::at(
                "branch",
                "chat.workspace.branch",
                "must be non-empty, at most 200 bytes, and contain no whitespace",
            ));
        }

        if self.message_prefixes.len() > 8 {
            return Err(ChatSettingsViolation::at(
                "message_prefixes",
                "chat.message_prefixes",
                "must contain at most 8 entries",
            ));
        }
        for (index, prefix) in self.message_prefixes.iter().enumerate() {
            if prefix.trim().is_empty() || prefix.len() > 256 || prefix.contains(['\r', '\n']) {
                return Err(ChatSettingsViolation::at(
                    "message_prefixes",
                    format!("chat.message_prefixes[{index}]"),
                    "entries must be non-empty, single-line, and at most 256 bytes",
                ));
            }
        }

        if !(60..=86_400).contains(&self.inactivity.pause_after_seconds) {
            return Err(ChatSettingsViolation::at(
                "pause_after_seconds",
                "chat.inactivity.pause_after_seconds",
                "must be from 60 through 86400 seconds",
            ));
        }

        if self.setup.commands.len() > 16 {
            return Err(ChatSettingsViolation::at(
                "commands",
                "chat.setup.commands",
                "must contain at most 16 entries",
            ));
        }
        for (index, command) in self.setup.commands.iter().enumerate() {
            if command.len() > 4_096 {
                return Err(ChatSettingsViolation::at(
                    "commands",
                    format!("chat.setup.commands[{index}]"),
                    "entries must be at most 4096 bytes",
                ));
            }
        }

        if !(1..=50).contains(&self.host.max_threads) {
            return Err(ChatSettingsViolation::at(
                "max_threads",
                "chat.host.max_threads",
                "must be from 1 through 50",
            ));
        }
        Ok(())
    }

    /// Derive the one directory name the runtime may create for this source.
    pub fn workspace_directory(&self) -> Result<String, ChatSettingsViolation> {
        let raw = &self.workspace.repository_url;
        let invalid = || {
            ChatSettingsViolation::at(
                "repository_url",
                "chat.workspace.repository_url",
                "must be a credential-free HTTPS repository URL of at most 512 bytes with a safe basename",
            )
        };
        if raw.len() > 512 {
            return Err(invalid());
        }
        let parsed = Url::parse(raw).map_err(|_| invalid())?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid());
        }
        let basename = parsed
            .path_segments()
            .and_then(|mut segments| segments.rfind(|part| !part.is_empty()))
            .map(|part| part.strip_suffix(".git").unwrap_or(part))
            .filter(|part| is_safe_segment(part))
            .ok_or_else(invalid)?;
        Ok(basename.to_string())
    }
}

fn is_safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The exact settings revision and derived workspace fact pinned by one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSettingsSnapshot {
    pub settings_revision: u64,
    pub settings: ChatSettings,
    pub workspace_directory: String,
}

impl ChatSettingsSnapshot {
    pub fn new(
        settings_revision: u64,
        settings: ChatSettings,
    ) -> Result<Self, ChatSettingsViolation> {
        settings.validate()?;
        let workspace_directory = settings.workspace_directory()?;
        Ok(Self {
            settings_revision,
            settings,
            workspace_directory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_complete_but_dark() {
        let settings: ChatSettings = serde_json::from_str("{}").expect("defaults deserialize");
        assert_eq!(settings, ChatSettings::default());
        assert!(!settings.enabled);
        assert_eq!(settings.message_prefixes, [DEFAULT_MESSAGE_PREFIX]);
        assert_eq!(settings.workspace_directory().unwrap(), "trureturing");
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn repository_urls_are_https_credential_free_and_path_safe() {
        for repository_url in [
            "http://example.test/repo",
            "https://user@example.test/repo",
            "https://example.test/..",
            "https://example.test/repo?token=hidden",
            "https://example.test/",
        ] {
            let settings = ChatSettings {
                workspace: ChatWorkspaceSource {
                    repository_url: repository_url.into(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let violation = settings.validate().expect_err("URL must be refused");
            assert_eq!(violation.field(), "repository_url");
            assert_eq!(violation.path(), "chat.workspace.repository_url");
            assert!(!violation.to_string().contains(repository_url));
        }
        let git_suffix = ChatSettings {
            workspace: ChatWorkspaceSource {
                repository_url: "https://example.test/team/project.git".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(git_suffix.workspace_directory().unwrap(), "project");
    }

    #[test]
    fn every_collection_and_numeric_bound_is_enforced() {
        let cases = [
            (
                ChatSettings {
                    message_prefixes: vec!["prefix".into(); 9],
                    ..Default::default()
                },
                "chat.message_prefixes",
            ),
            (
                ChatSettings {
                    message_prefixes: vec![" \t".into()],
                    ..Default::default()
                },
                "chat.message_prefixes[0]",
            ),
            (
                ChatSettings {
                    inactivity: ChatInactivity {
                        pause_after_seconds: 59,
                    },
                    ..Default::default()
                },
                "chat.inactivity.pause_after_seconds",
            ),
            (
                ChatSettings {
                    setup: ChatSetup {
                        commands: vec![String::new(); 17],
                    },
                    ..Default::default()
                },
                "chat.setup.commands",
            ),
            (
                ChatSettings {
                    host: ChatHostSettings {
                        max_threads: 51,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                "chat.host.max_threads",
            ),
        ];
        for (settings, path) in cases {
            assert_eq!(settings.validate().unwrap_err().path(), path);
        }
    }

    #[test]
    fn the_snapshot_is_a_byte_stable_resolved_copy() {
        let settings = ChatSettings {
            enabled: true,
            setup: ChatSetup {
                commands: vec!["tool --install".into()],
            },
            ..Default::default()
        };
        let snapshot = ChatSettingsSnapshot::new(7, settings).expect("valid snapshot");
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_slice::<ChatSettingsSnapshot>(&bytes).unwrap(),
            snapshot
        );
        assert_eq!(snapshot.workspace_directory, "trureturing");
    }
}
