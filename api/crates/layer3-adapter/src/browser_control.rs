use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserControlCommand {
    Status,
    Acquire,
    Input,
    Renew,
    Return,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserInput {
    Click { x: u16, y: u16 },
    Text { text: String },
    Key { key: String },
}

impl std::fmt::Debug for BrowserInput {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("BrowserInput([redacted])")
    }
}

impl BrowserInput {
    pub fn valid(&self) -> bool {
        match self {
            Self::Click { x, y } => *x < 1280 && *y < 720,
            Self::Text { text } => {
                !text.is_empty() && text.len() <= 4096 && text.encode_utf16().count() <= 1024
            }
            Self::Key { key } => matches!(
                key.as_str(),
                "Backspace"
                    | "Tab"
                    | "Enter"
                    | "Escape"
                    | "Home"
                    | "End"
                    | "ArrowLeft"
                    | "ArrowUp"
                    | "ArrowRight"
                    | "ArrowDown"
                    | "Delete"
            ),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserControlRequest {
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    pub agent_id: crate::AgentSlot,
    pub context_id: Option<String>,
    pub actor: String,
    pub client_id: String,
    pub command: BrowserControlCommand,
    pub operation_id: Option<String>,
    pub expected_revision: Option<u64>,
    pub lease_token: Option<String>,
    pub input: Option<BrowserInput>,
}

impl std::fmt::Debug for BrowserControlRequest {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("BrowserControlRequest")
            .field("agent_id", &self.agent_id)
            .field("command", &self.command)
            .field("operation_id", &self.operation_id)
            .finish_non_exhaustive()
    }
}

impl BrowserControlRequest {
    pub fn valid(&self) -> bool {
        let identifier = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        };
        identifier(&self.actor)
            && identifier(&self.client_id)
            && self.client_id.len() <= 64
            && self
                .context_id
                .as_ref()
                .is_none_or(|value| uuid::Uuid::parse_str(value).is_ok())
            && self
                .operation_id
                .as_ref()
                .is_none_or(|value| uuid::Uuid::parse_str(value).is_ok())
            && self
                .lease_token
                .as_ref()
                .is_none_or(|value| uuid::Uuid::parse_str(value).is_ok())
            && if self.command == BrowserControlCommand::Status {
                self.input.is_none()
                    && self.lease_token.is_none()
                    && self.expected_revision.is_none()
            } else {
                self.context_id.is_some()
                    && self.operation_id.is_some()
                    && self.expected_revision.is_some()
                    && (self.command == BrowserControlCommand::Acquire
                        || self.lease_token.is_some())
                    && if self.command == BrowserControlCommand::Input {
                        self.input.as_ref().is_some_and(BrowserInput::valid)
                    } else {
                        self.input.is_none()
                    }
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserControlState {
    Agent,
    User,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserControlAudit {
    pub revision: u64,
    pub kind: String,
    pub actor: String,
    pub client_id: String,
    pub operation_id: Option<String>,
    pub at: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserControlStatus {
    pub state: BrowserControlState,
    pub revision: u64,
    pub client_id: Option<String>,
    pub lease_token: Option<String>,
    pub expires_at: Option<String>,
    pub lease_duration_ms: u32,
    pub operation_id: Option<String>,
    pub operation_revision: Option<u64>,
    pub audit: Vec<BrowserControlAudit>,
    pub audit_truncated_before: Option<u64>,
}

impl std::fmt::Debug for BrowserControlStatus {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("BrowserControlStatus")
            .field("state", &self.state)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl BrowserControlStatus {
    pub fn valid(&self) -> bool {
        self.lease_duration_ms == 15000
            && self.audit.len() <= 128
            && self
                .lease_token
                .as_ref()
                .is_none_or(|value| uuid::Uuid::parse_str(value).is_ok())
            && self
                .client_id
                .as_ref()
                .is_none_or(|value| value.len() <= 64)
            && self
                .expires_at
                .as_ref()
                .is_none_or(|value| value.len() <= 32)
            && (self.state != BrowserControlState::User
                || (self.client_id.is_some() && self.expires_at.is_some()))
            && self.audit.iter().all(|event| {
                event.revision <= self.revision
                    && matches!(
                        event.kind.as_str(),
                        "acquired" | "input" | "returned" | "renewed" | "expired" | "unknown"
                    )
                    && event.actor.len() <= 128
                    && event.client_id.len() <= 64
                    && event.at.len() <= 32
                    && event
                        .operation_id
                        .as_ref()
                        .is_none_or(|value| uuid::Uuid::parse_str(value).is_ok())
            })
            && self
                .audit
                .windows(2)
                .all(|events| events[0].revision < events[1].revision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserControlReceipt {
    pub agent_id: crate::AgentSlot,
    pub boot_id: String,
    pub context_id: String,
    pub control: BrowserControlStatus,
}
