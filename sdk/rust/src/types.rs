//! REST request and response types.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Stable identifier of a CMA session.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Validate and retain a session identifier.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if !value.starts_with("sesn_") || value.len() <= "sesn_".len() {
            return Err(Error::protocol(
                "a session id must start with 'sesn_' and carry a suffix",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(Error::protocol(
                "a session id contains a character outside the wire identifier alphabet",
            ));
        }
        Ok(Self(value))
    }

    /// Borrow the wire identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SessionId").field(&self.0).finish()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl TryFrom<String> for SessionId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Non-zero terminal dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGeometry {
    /// Width in character columns.
    pub cols: u16,
    /// Height in character rows.
    pub rows: u16,
}

impl TerminalGeometry {
    /// Build terminal dimensions, rejecting a zero boundary.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        if cols == 0 || rows == 0 {
            return Err(Error::protocol(
                "terminal geometry requires non-zero columns and rows",
            ));
        }
        Ok(Self { cols, rows })
    }

    pub(crate) fn validate(self, boundary: &str) -> Result<Self> {
        Self::new(self.cols, self.rows)
            .map_err(|_| Error::protocol(format!("{boundary} contains zero terminal geometry")))
    }
}

/// Parameters accepted when creating a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateSessionRequest {
    /// Initial terminal dimensions, or the deployment default when absent.
    pub geometry: Option<TerminalGeometry>,
    /// Environment profile used to configure the new session.
    pub environment_profile: Option<String>,
    /// Sealed rate-card identifier accepted by the caller.
    pub quoted_rate_card: Option<String>,
}

/// Input delivered to a terminal or conversation session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionInput {
    /// Text input, optionally followed by a newline on the terminal plane.
    Text {
        /// Text to deliver verbatim.
        text: String,
        /// Whether a terminal should receive a trailing newline.
        newline: bool,
    },
    /// Raw terminal bytes, encoded as base64 by the client.
    Bytes(Vec<u8>),
}

impl SessionInput {
    /// Construct text input using CMA's line-submission default.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            newline: true,
        }
    }

    /// Construct text input without submitting a terminal line.
    pub fn text_without_newline(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            newline: false,
        }
    }

    /// Construct raw terminal input.
    pub fn bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self::Bytes(bytes.into())
    }
}

/// An answer to a pending approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Permit the requested action.
    Allow,
    /// Refuse this action and let the turn continue.
    Deny,
    /// Refuse this action and abandon the turn.
    Abort,
}

/// Lifecycle position of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The runtime is being provisioned.
    Provisioning,
    /// The runtime is ready for work.
    Ready,
    /// A turn is in progress.
    Running,
    /// The session is being carried to another runtime.
    Rotating,
    /// The runtime was reclaimed and can be revived.
    Idle,
    /// No further work can occur under this session id.
    Terminated,
}

/// Operational health reported beside lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    /// Nothing unusual is known.
    Normal,
    /// Reclaimed and revivable.
    Idle,
    /// Being restored.
    Recovering,
    /// Running with reduced capability.
    Degraded,
    /// The runtime cannot currently be observed.
    Unknown,
    /// Permanently withdrawn from service.
    Retired,
    /// Configuration is unusable.
    Invalid,
}

/// Why a session stopped or paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// A turn finished normally.
    EndTurn,
    /// The owner terminated the session.
    UserTerminated,
    /// The runtime was reclaimed after idling.
    IdleTimeout,
    /// The session reached its maximum lifetime.
    LifetimeCap,
    /// The runtime is waiting for owner action and is not terminal.
    RequiresAction,
    /// Credentials or authorization were withdrawn.
    Revoked,
    /// A spend or usage ceiling was reached.
    BudgetReached,
    /// A failure stopped the session.
    Error,
}

/// How much the agent may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Ask before reaching outside the workspace.
    Guarded,
    /// Act inside the workspace and ask at its boundary.
    Autonomous,
    /// Ask for nothing and remove internal confinement.
    Unrestricted,
}

/// How much model deliberation the session requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Barely any deliberation.
    Minimal,
    /// Low deliberation.
    Low,
    /// Medium deliberation.
    Medium,
    /// High deliberation.
    High,
    /// The highest deliberation the model offers.
    XHigh,
}

/// Whose account pays for model calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// The platform acts on the owner's delegated authority.
    Delegated,
    /// The owner signs the agent into their own vendor account.
    Own,
}

/// Runtime reservation attached to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSize {
    /// Processor reservation in thousandths of a core.
    pub cpu_millis: u32,
    /// Working-memory reservation in mebibytes.
    pub memory_mib: u32,
}

/// One skill installed in a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Stable skill identifier.
    pub id: String,
    /// Installed version, when recorded.
    #[serde(default)]
    pub version: Option<String>,
    /// Human-facing registry name, when available.
    #[serde(default)]
    pub name: Option<String>,
    /// Human-facing registry description, when available.
    #[serde(default)]
    pub description: Option<String>,
}

/// Agent configuration fixed for a session's life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Selected model, or the deployment default when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// Permission boundary granted to the agent.
    pub permission: PermissionMode,
    /// Requested model deliberation.
    pub reasoning_effort: ReasoningEffort,
    /// Credential source used for model calls.
    pub credential: CredentialMode,
    /// Runtime reservation, or the deployment reservation when absent.
    #[serde(default)]
    pub size: Option<SessionSize>,
    /// Installed skills; absence means the grant predates this field.
    #[serde(default)]
    pub skills: Option<Vec<Skill>>,
}

/// A service a session may call for its owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    /// Stable service identifier.
    pub id: String,
    /// Human-facing service name, when the catalogue is reachable.
    #[serde(default)]
    pub name: Option<String>,
    /// Human-facing service description, when available.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the platform mandates this service.
    pub mandated: bool,
}

/// Public identity of a session creator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicIdentity {
    /// Display name, when the identity provider released one.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Absolute avatar URL, when one was released.
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Price sealed onto a session at creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPricing {
    /// Stable rate-card identifier.
    pub rate_card: String,
    /// ISO currency code.
    pub currency: String,
    /// Micro-units per hour as a decimal string.
    pub micros_per_hour: String,
    /// Server-rendered amount for display.
    pub display: String,
    /// Time the quote was issued, in RFC 3339 form.
    pub quoted_at: String,
}

/// A CMA session as returned by create, list, and read operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// Stable session identifier.
    pub id: SessionId,
    /// Lifecycle position.
    pub status: SessionStatus,
    /// Operational health.
    pub recovery_state: RecoveryState,
    /// Why the session stopped or paused, when known.
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    /// Kernel serving the session.
    pub kernel: String,
    /// Agent configuration fixed at creation.
    pub agent: Agent,
    /// Terminal width.
    pub cols: u16,
    /// Terminal height.
    pub rows: u16,
    /// Highest durable event sequence written for this session.
    pub last_seq: u64,
    /// Creation time in RFC 3339 form.
    pub created_at: String,
    /// Runtime image version, once a rotation has occurred.
    #[serde(default)]
    pub runtime_version: Option<String>,
    /// Number of runtime rotations.
    #[serde(default)]
    pub rotations: u32,
    /// Snapshot moment used by the current runtime.
    #[serde(default)]
    pub rotated_at: Option<String>,
    /// Creator identity, when the caller may see it.
    #[serde(default)]
    pub creator: Option<PublicIdentity>,
    /// Conformance classes granted to this session.
    pub conforms_to: Vec<String>,
    /// Services this session may reach.
    pub reach: Vec<Reach>,
    /// Reserved-capacity price fixed at creation.
    #[serde(default)]
    pub pricing: Option<SessionPricing>,
}

impl Session {
    pub(crate) fn validate(self) -> Result<Self> {
        TerminalGeometry::new(self.cols, self.rows)
            .map_err(|_| Error::protocol("a session response contains zero terminal geometry"))?;
        if self
            .agent
            .size
            .is_some_and(|size| size.cpu_millis == 0 || size.memory_mib == 0)
        {
            return Err(Error::protocol(
                "a session response contains a zero runtime reservation",
            ));
        }
        if self.kernel.trim().is_empty() {
            return Err(Error::protocol(
                "a session response contains an empty kernel",
            ));
        }
        if self.created_at.trim().is_empty() {
            return Err(Error::protocol(
                "a session response contains an empty creation timestamp",
            ));
        }
        Ok(self)
    }
}

/// Current rendered terminal state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    /// Width in character columns.
    pub cols: u16,
    /// Height in character rows.
    pub rows: u16,
    /// Rendered rows in display order.
    pub lines: Vec<String>,
}

impl ScreenSnapshot {
    pub(crate) fn validate(self) -> Result<Self> {
        TerminalGeometry::new(self.cols, self.rows)
            .map_err(|_| Error::protocol("a screen response contains zero terminal geometry"))?;
        if self.lines.len() != usize::from(self.rows) {
            return Err(Error::protocol(format!(
                "a screen declared {} rows but carried {} lines",
                self.rows,
                self.lines.len()
            )));
        }
        Ok(self)
    }
}

/// Token and context usage reported by a session runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens sent to the model.
    pub input_tokens: u64,
    /// Tokens produced by the model.
    pub output_tokens: u64,
    /// Tokens currently occupying the context.
    pub context_used: u64,
    /// Context ceiling, when the runtime states one.
    #[serde(default)]
    pub context_window: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_identifier_and_input_helpers_preserve_values() {
        let id = SessionId::new("sesn_fixture").expect("valid id");
        assert_eq!(id.as_ref(), "sesn_fixture");
        assert_eq!(id.to_string(), "sesn_fixture");
        assert!(format!("{id:?}").contains("sesn_fixture"));
        assert_eq!(
            SessionId::try_from("sesn_other".to_owned())
                .expect("valid conversion")
                .as_str(),
            "sesn_other"
        );
        assert!(matches!(
            SessionInput::text("line"),
            SessionInput::Text { newline: true, .. }
        ));
        assert!(matches!(
            SessionInput::bytes([1, 2]),
            SessionInput::Bytes(bytes) if bytes == [1, 2]
        ));
    }

    #[test]
    fn response_validators_reject_impossible_boundaries() {
        let screen = ScreenSnapshot {
            cols: 80,
            rows: 2,
            lines: vec!["only one".into()],
        };
        assert!(screen.validate().is_err());

        let mut session: Session =
            serde_json::from_str(include_str!("../tests/fixtures/session.json"))
                .expect("fixture is a session");
        session.agent.size = Some(SessionSize {
            cpu_millis: 0,
            memory_mib: 1,
        });
        assert!(session.validate().is_err());
    }
}
