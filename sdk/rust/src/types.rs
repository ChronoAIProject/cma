//! REST request and response types.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Anonymous deployment conformance and build discovery document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conformance {
    /// Specification conformance classes implemented by this deployment.
    #[serde(rename = "conformsTo")]
    pub conforms_to: Vec<String>,
    /// Deployment version string.
    pub version: String,
    /// Source revision used for this deployment.
    pub build_sha: String,
}

impl Conformance {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.conforms_to.is_empty()
            || self
                .conforms_to
                .iter()
                .any(|conformance| conformance.trim().is_empty())
        {
            return Err(Error::protocol(
                "conformance response requires non-empty conformance identifiers",
            ));
        }
        if self.version.trim().is_empty() || self.build_sha.trim().is_empty() {
            return Err(Error::protocol(
                "conformance response requires non-empty version and build_sha",
            ));
        }
        Ok(self)
    }
}

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

/// Stable identifier of a deployment-managed shared chat host.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ChatHostId(String);

impl ChatHostId {
    /// Validate and retain a chat-host identifier.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if !value.starts_with("chho_") || value.len() <= "chho_".len() {
            return Err(Error::protocol(
                "a chat host id must start with 'chho_' and carry a suffix",
            ));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(Error::protocol(
                "a chat host id contains a character outside the wire identifier alphabet",
            ));
        }
        Ok(Self(value))
    }

    /// Borrow the wire identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ChatHostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ChatHostId").field(&self.0).finish()
    }
}

impl fmt::Display for ChatHostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for ChatHostId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl TryFrom<String> for ChatHostId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ChatHostId {
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

/// Deployment-managed surface selected when a session is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSurface {
    /// A worktree-backed thread on the owner's shared chat host.
    Chat,
}

/// Parameters accepted when creating a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateSessionRequest {
    /// Deployment-managed surface, or an ordinary agent session when absent.
    pub surface: Option<SessionSurface>,
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

impl SessionStatus {
    pub(crate) const fn query_value(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Rotating => "rotating",
            Self::Idle => "idle",
            Self::Terminated => "terminated",
        }
    }
}

/// Session family accepted by the bounded session collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSurfaceFilter {
    /// Worktree-backed shared-host chat threads.
    Chat,
    /// Ordinary independently provisioned agent sessions.
    Agent,
}

impl SessionSurfaceFilter {
    pub(crate) const fn query_value(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Agent => "agent",
        }
    }
}

/// Smallest page accepted by CMA's bounded collections.
pub const MIN_PAGE_SIZE: u16 = 1;

/// Largest page accepted by CMA's bounded collections.
pub const MAX_PAGE_SIZE: u16 = 200;

/// Opaque, route-bound target for the next page returned by CMA.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PageContinuation(String);

impl PageContinuation {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Borrow the opaque target for logging or persistence without interpreting it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One bounded collection response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// Validated rows in stable server order.
    pub items: Vec<T>,
    /// Route-bound target for another page, when more rows exist.
    pub next: Option<PageContinuation>,
}

/// Exact filter and position for one bounded session page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionPageRequest {
    status: Option<SessionStatus>,
    surface: Option<SessionSurfaceFilter>,
    limit: Option<u16>,
    cursor: Option<String>,
    continuation: Option<PageContinuation>,
}

impl SessionPageRequest {
    /// Start a first-page request with no filter and the server's default limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Narrow the first page to one exact lifecycle state.
    pub fn with_status(mut self, status: SessionStatus) -> Result<Self> {
        if self.continuation.is_some() {
            return Err(Error::protocol(
                "a pagination continuation cannot be combined with a status filter",
            ));
        }
        self.status = Some(status);
        Ok(self)
    }

    /// Narrow the first page to chat threads or ordinary agent sessions.
    pub fn with_surface(mut self, surface: SessionSurfaceFilter) -> Result<Self> {
        if self.continuation.is_some() {
            return Err(Error::protocol(
                "a pagination continuation cannot be combined with a surface filter",
            ));
        }
        self.surface = Some(surface);
        Ok(self)
    }

    /// Set a first-page size from 1 through 200.
    pub fn with_limit(mut self, limit: u16) -> Result<Self> {
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::protocol(format!(
                "session page limit must be from {MIN_PAGE_SIZE} through {MAX_PAGE_SIZE}"
            )));
        }
        if self.continuation.is_some() {
            return Err(Error::protocol(
                "a pagination continuation cannot be combined with a page limit",
            ));
        }
        self.limit = Some(limit);
        Ok(self)
    }

    /// Resume a first-page query from an opaque cursor supplied out of band.
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Result<Self> {
        let cursor = cursor.into();
        if cursor.is_empty() {
            return Err(Error::protocol("session page cursor must not be empty"));
        }
        if self.continuation.is_some() {
            return Err(Error::protocol(
                "a pagination continuation cannot be combined with a cursor",
            ));
        }
        self.cursor = Some(cursor);
        Ok(self)
    }

    /// Follow a route-bound continuation returned by [`Page::next`].
    pub fn continuing(continuation: PageContinuation) -> Self {
        Self {
            continuation: Some(continuation),
            ..Self::default()
        }
    }

    /// Exact lifecycle filter, when this is a first-page request.
    pub fn status(&self) -> Option<SessionStatus> {
        self.status
    }

    /// Exact session-family filter, when this is a first-page request.
    pub fn surface(&self) -> Option<SessionSurfaceFilter> {
        self.surface
    }

    /// Requested first-page size, or `None` for the server default.
    pub fn limit(&self) -> Option<u16> {
        self.limit
    }

    /// Opaque first-page cursor supplied by the caller.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Server-issued continuation target, when following a previous page.
    pub fn continuation(&self) -> Option<&PageContinuation> {
        self.continuation.as_ref()
    }
}

/// Exact position and size for one bounded administrative chat-host page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatHostPageRequest {
    limit: Option<u16>,
    cursor: Option<String>,
    continuation: Option<PageContinuation>,
}

impl ChatHostPageRequest {
    /// Start a first-page request using the server's default limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a first-page size from 1 through 200.
    pub fn with_limit(mut self, limit: u16) -> Result<Self> {
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::protocol(format!(
                "chat host page limit must be from {MIN_PAGE_SIZE} through {MAX_PAGE_SIZE}"
            )));
        }
        if self.continuation.is_some() {
            return Err(Error::protocol(
                "a pagination continuation cannot be combined with a page limit",
            ));
        }
        self.limit = Some(limit);
        Ok(self)
    }

    /// Resume a first-page query from an opaque cursor supplied out of band.
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Result<Self> {
        let cursor = cursor.into();
        if cursor.is_empty() {
            return Err(Error::protocol("chat host page cursor must not be empty"));
        }
        if self.continuation.is_some() {
            return Err(Error::protocol(
                "a pagination continuation cannot be combined with a cursor",
            ));
        }
        self.cursor = Some(cursor);
        Ok(self)
    }

    /// Follow a route-bound continuation returned by [`Page::next`].
    pub fn continuing(continuation: PageContinuation) -> Self {
        Self {
            continuation: Some(continuation),
            ..Self::default()
        }
    }

    /// Requested first-page size, or `None` for the server default.
    pub fn limit(&self) -> Option<u16> {
        self.limit
    }

    /// Opaque first-page cursor supplied by the caller.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Server-issued continuation target, when following a previous page.
    pub fn continuation(&self) -> Option<&PageContinuation> {
        self.continuation.as_ref()
    }
}

/// Finite resource ceilings for the compatibility list-all session operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageWalkLimits {
    max_pages: usize,
    max_items: usize,
}

impl PageWalkLimits {
    /// Build non-zero page and retained-item ceilings.
    pub fn new(max_pages: usize, max_items: usize) -> Result<Self> {
        if max_pages == 0 || max_items == 0 {
            return Err(Error::protocol(
                "pagination walk limits must both be non-zero",
            ));
        }
        Ok(Self {
            max_pages,
            max_items,
        })
    }

    /// Maximum responses accepted by one list-all operation.
    pub fn max_pages(self) -> usize {
        self.max_pages
    }

    /// Maximum validated rows retained by one list-all operation.
    pub fn max_items(self) -> usize {
        self.max_items
    }
}

impl Default for PageWalkLimits {
    fn default() -> Self {
        Self {
            max_pages: 100,
            max_items: 20_000,
        }
    }
}

/// Lifecycle of a deployment-managed shared chat runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatHostStatus {
    /// The initial shared runtime is being created.
    Provisioning,
    /// The runtime can serve chat threads.
    Active,
    /// Durable inactivity capture is in progress.
    Pausing,
    /// The runtime is reclaimed and can be revived.
    Paused,
    /// A replacement runtime is being restored.
    Reviving,
    /// The host was administratively withdrawn.
    Retired,
}

/// Phase of one sanitized failure retained for chat-host operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatHostFailurePhase {
    /// Provider runtime creation or readiness failed.
    Runtime,
    /// The configured repository could not be prepared.
    WorkspaceSource,
    /// An administrator-owned setup command or its transport failed.
    WorkspaceSetup,
    /// The owner's delegated credential could not be installed.
    Credential,
    /// A failed host transition still needs runtime cleanup.
    Cleanup,
    /// The recorded runtime stopped serving unexpectedly.
    RuntimeLost,
}

/// Bounded operator evidence for the host's most recent failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatHostFailure {
    /// Typed lifecycle phase that failed.
    pub phase: ChatHostFailurePhase,
    /// Zero-based setup command index, when a command returned nonzero.
    #[serde(default)]
    pub command_index: Option<usize>,
    /// Combined stdout/stderr tail, bounded by the runtime to 32 KiB.
    #[serde(default)]
    pub output_excerpt: Option<String>,
    /// Whether retrying the host transition may succeed without policy changes.
    pub retryable: bool,
    /// Whether the failed transition still owns a runtime that needs removal.
    pub cleanup_pending: bool,
    /// Time this failure was durably recorded, in RFC 3339 form.
    pub occurred_at: String,
}

/// Redacted administrative projection of one shared chat host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatHost {
    /// Stable host identifier.
    pub id: ChatHostId,
    /// Subject that owns the shared host.
    pub owner: String,
    /// Current host lifecycle position.
    pub status: ChatHostStatus,
    /// Number of retained, non-removed chat threads.
    pub thread_count: usize,
    /// Runtime image version, when known.
    #[serde(default)]
    pub runtime_version: Option<String>,
    /// Most recent successful pause time, in RFC 3339 form.
    #[serde(default)]
    pub paused_at: Option<String>,
    /// Most recent successful revival time, in RFC 3339 form.
    #[serde(default)]
    pub revived_at: Option<String>,
    /// Time of the retained verified checkpoint, in RFC 3339 form.
    #[serde(default)]
    pub checkpoint_captured_at: Option<String>,
    /// Most recent sanitized host failure, when one is retained.
    #[serde(default)]
    pub last_failure: Option<ChatHostFailure>,
    /// Platform-settings revision pinned when this host generation was created.
    pub settings_revision: u64,
    /// Host-generation creation time, in RFC 3339 form.
    pub created_at: String,
}

impl ChatHost {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.owner.trim().is_empty() {
            return Err(Error::protocol(
                "a chat host response contains an empty owner",
            ));
        }
        if self
            .runtime_version
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(Error::protocol(
                "a chat host response contains an empty runtime version",
            ));
        }
        for (value, field) in [
            (self.paused_at.as_deref(), "pause"),
            (self.revived_at.as_deref(), "revival"),
            (self.checkpoint_captured_at.as_deref(), "checkpoint"),
        ] {
            if value.is_some_and(|timestamp| timestamp.trim().is_empty()) {
                return Err(Error::protocol(format!(
                    "a chat host response contains an empty {field} timestamp"
                )));
            }
        }
        if self.created_at.trim().is_empty() {
            return Err(Error::protocol(
                "a chat host response contains an empty creation timestamp",
            ));
        }
        if let Some(failure) = self.last_failure.as_ref()
            && (failure.occurred_at.trim().is_empty()
                || failure
                    .output_excerpt
                    .as_ref()
                    .is_some_and(|excerpt| excerpt.len() > 32 * 1024)
                || (failure.command_index.is_some()
                    && failure.phase != ChatHostFailurePhase::WorkspaceSetup)
                || (failure.output_excerpt.is_some()
                    && (failure.phase != ChatHostFailurePhase::WorkspaceSetup
                        || failure.command_index.is_none())))
        {
            return Err(Error::protocol(
                "a chat host response contains a malformed failure",
            ));
        }
        Ok(self)
    }
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

/// Shared-host worktree facts returned for a chat thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    /// Stable worktree allocation name within the shared host.
    pub worktree_name: String,
    /// Runtime path used as this conversation's working directory.
    pub worktree_path: String,
    /// Generated display title, when one has been committed.
    #[serde(default)]
    pub title: Option<String>,
    /// Human-facing pause explanation, when the host is paused.
    #[serde(default)]
    pub paused_notice: Option<String>,
}

impl ChatSession {
    fn validate(&self) -> Result<()> {
        if self.worktree_name.trim().is_empty() {
            return Err(Error::protocol(
                "a chat session response contains an empty worktree name",
            ));
        }
        if self.worktree_path.trim().is_empty() {
            return Err(Error::protocol(
                "a chat session response contains an empty worktree path",
            ));
        }
        Ok(())
    }
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
    /// Shared-host worktree facts for a chat thread.
    #[serde(default)]
    pub chat: Option<ChatSession>,
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
        if let Some(chat) = &self.chat {
            chat.validate()?;
        }
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
