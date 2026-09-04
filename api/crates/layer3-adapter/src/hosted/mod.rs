//! A kernel hosted in a provider-managed sandbox.
//!
//! Shared machinery, like `sandbox` and `terminal`: everything about running a
//! kernel in a pod that does not depend on which kernel it is. Bringing a
//! sandbox up and waiting for it, handing a conversation to the one that
//! replaces it, capturing one from the one being replaced, the link back and
//! the credential that rides on it — none of that changes with the agent
//! inside.
//!
//! It exists because the second kernel arrived. The first one's directory held
//! all of this, which looked right until there was something to compare it
//! against: a kernel's directory should hold what is true of that kernel and
//! nothing else, and none of this was.
//!
//! What a kernel's own directory keeps is small and real — what its sandbox is
//! shaped like, what environment its agent needs, what it can actually do, and
//! the words its bridge uses that no other bridge does.

mod acp;
mod deployment;
mod handshake;
mod inbound;
mod link;
pub(crate) use link::credential_answer;
mod loss;
mod relay;
mod telemetry;
pub(super) mod turn;

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod evidence_tests;

use std::collections::BTreeMap;
use std::io::{Read as _, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::ZlibDecoder;
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::sandbox_reclaimer::{
    CREATOR_METADATA_KEY, GENERATION_METADATA_KEY, KERNEL_METADATA_KEY, LEGACY_KERNEL_METADATA_KEY,
    RESERVATION_METADATA_KEY, SESSION_METADATA_KEY,
};
use crate::task_retirement::TaskRetirement;
use crate::terminal_input::WeakTerminalInputSender;
use crate::{
    AdapterError, ConversationChannel, ConversationUpdate, RuntimeCapacity, RuntimeGeneration,
    RuntimeHandle, RuntimeIdentity, RuntimeListing, RuntimeReservationIdentity, RuntimeSpec,
    SkillPackage, TerminalInput, TerminalInputSender, WorkspaceCheckpointDisposition,
    WorkspaceCheckpointPackage, WorkspaceCheckpointRequest, WorkspaceEntryKind,
    WorkspaceEntryPackage, WorkspacePackage, WorkspaceSetupReceipt, WorkspaceSourceRequest,
    WorkspaceTreeReceipt, WorkspaceTreeRequest, WorkspaceWriterState,
    HELD_CHECKPOINT_RECOVERY_ERROR,
};

pub use deployment::{Deployment, PLACEHOLDER_CREDENTIAL};

/// The shared evidence semantics used by a hosted conversation protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationEvidenceContract {
    /// ACP v1 updates use [`crate::ConversationUpdate::turn_evidence`].
    AcpV1,
}

/// Kernel-specific values supplied to the one hosted ACP conversation path.
///
/// The protocol, frame translation, and evidence classification are not
/// kernel-specific. Keeping those behind this typed binding makes it possible
/// for Codex and Claude Code to choose their own permission mode without
/// silently acquiring different prompt-delivery semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcpConversationBinding {
    mode: Option<String>,
    resume_conversation: Option<String>,
    own_credential: bool,
    working_directory: Option<String>,
    conversation_slot: Option<String>,
}

impl AcpConversationBinding {
    /// Bind one kernel's mode and credential behavior to hosted ACP v1.
    pub(crate) fn new(
        mode: Option<String>,
        resume_conversation: Option<String>,
        own_credential: bool,
        working_directory: Option<String>,
        conversation_slot: Option<String>,
    ) -> Self {
        Self {
            mode,
            resume_conversation,
            own_credential,
            working_directory,
            conversation_slot,
        }
    }

    /// The evidence contract production opening will use for this binding.
    pub(crate) fn evidence_contract(&self) -> ConversationEvidenceContract {
        ConversationEvidenceContract::AcpV1
    }
}

/// Port the supervisor listens on inside the sandbox.
pub(crate) const SUPERVISOR_PORT: u16 = 9090;

/// Canonical metadata key naming the control plane that created a sandbox.
///
/// Written at provisioning and read by reconciliation, so a provider shared
/// between control planes can be told apart. Absent on sandboxes created before
/// this was recorded, which reconciliation treats as its own business to decide.
/// Historical control-plane key, kept while old-only sandboxes remain live.
const LEGACY_CREATOR_METADATA_KEY: &str = "chrono-bot/control-plane";

/// Canonical metadata key correlating a sandbox back to its session.
///
/// Written at provisioning and read by reconciliation, so the two must agree;
/// naming it once is what keeps them in step.
/// Historical session key, kept while old-only sandboxes remain live.
const LEGACY_SESSION_METADATA_KEY: &str = "chrono-bot/session";
const CHAT_HOST_METADATA_KEY: &str = "cma/chat-host";

/// Read a canonical correlation value, falling back only when it is absent.
fn correlation_value(
    metadata: &BTreeMap<String, String>,
    canonical: &str,
    legacy: &str,
) -> Option<String> {
    metadata
        .get(canonical)
        .or_else(|| metadata.get(legacy))
        .cloned()
}

/// Decode a generation without conflating corrupt metadata with legacy absence.
fn runtime_generation(metadata: &BTreeMap<String, String>) -> RuntimeGeneration {
    match metadata.get(GENERATION_METADATA_KEY) {
        None => RuntimeGeneration::Legacy,
        Some(value) => value
            .parse()
            .ok()
            .and_then(std::num::NonZeroU64::new)
            .map_or(RuntimeGeneration::Malformed, RuntimeGeneration::Recorded),
    }
}

/// Give new and old control planes the same ownership facts during rollout.
fn provisioning_metadata(spec: &RuntimeSpec, kernel: &str) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::from([
        (SESSION_METADATA_KEY.to_string(), spec.session_id.clone()),
        (
            LEGACY_SESSION_METADATA_KEY.to_string(),
            spec.session_id.clone(),
        ),
        (KERNEL_METADATA_KEY.to_string(), kernel.to_string()),
        (LEGACY_KERNEL_METADATA_KEY.to_string(), kernel.to_string()),
    ]);
    if let Some(generation) = spec.generation {
        metadata.insert(GENERATION_METADATA_KEY.to_string(), generation.to_string());
    }
    if let Some(chat_host) = spec.chat_host.as_ref() {
        metadata.insert(CHAT_HOST_METADATA_KEY.to_string(), chat_host.clone());
    }
    if let Some(reservation) = spec.reservation_token.as_ref() {
        metadata.insert(RESERVATION_METADATA_KEY.to_string(), reservation.clone());
    }
    if let Some(creator) = spec.created_by.as_ref() {
        metadata.insert(CREATOR_METADATA_KEY.to_string(), creator.clone());
        metadata.insert(LEGACY_CREATOR_METADATA_KEY.to_string(), creator.clone());
    }
    metadata
}

/// Where the supervisor serves and accepts the agent's conversation.
const CONVERSATION_PATH: &str = "/runtime/conversation";

/// Where a pod takes the files its kernel needs before starting.
const PRELUDE_PATH: &str = "/runtime/prelude";
/// Exact maximum accepted by the runtime supervisor's prelude parser.
const PRELUDE_LIMIT: usize = 4 * 1024 * 1024;
/// Skill files plus the one optional standing-instructions document.
const PRELUDE_FILE_LIMIT: usize = 4_097;
/// Largest byte length of one component accepted by the Linux runtime image.
const PRELUDE_COMPONENT_LIMIT: usize = 255;

/// Versioned repository workspace restore/checkpoint endpoint.
const WORKSPACE_PATH: &str = "/runtime/workspace";
const WORKSPACE_CHECKPOINT_PATH: &str = "/runtime/workspace/checkpoint";
const WORKSPACE_CHECKPOINT_RESUME_PATH: &str = "/runtime/workspace/checkpoint/resume";
const WORKSPACE_CHECKPOINT_PREEMPT_PATH: &str = "/runtime/workspace/checkpoint/preempt";
const WORKSPACE_SOURCE_PATH: &str = "/runtime/workspace/source";
const WORKSPACE_TREES_PATH: &str = "/runtime/workspace/trees";
const WORKSPACE_SETUP_PATH: &str = "/runtime/workspace/setup";
const RUNTIME_GATEWAY_PROBE_PATH: &str = "/runtime/gateway-probe";
const WORKSPACE_VERSION: u64 = 2;
const WORKSPACE_HOLD_VERSION: u64 = 6;
const WORKSPACE_PREEMPT_VERSION: u64 = 7;
const WORKSPACE_COMPLETE_GIT_VERSION: u64 = 5;
const WORKSPACE_SOURCE_RECEIPT_VERSION: u64 = 2;
const WORKSPACE_ARCHIVE_VERSION: u32 = 2;
const LEGACY_WORKSPACE_ARCHIVE_VERSION: u32 = 1;
const MAX_WORKSPACE_FILES: usize = 750_000;
const MAX_WORKSPACE_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_WORKSPACE_HEADER_BYTES: usize = 128 * 1024 * 1024;
const MAX_WORKSPACE_INLINE_BYTES: u64 = 1_024;
const MAX_WORKSPACE_RESPONSE_BYTES: usize =
    MAX_WORKSPACE_BYTES + MAX_WORKSPACE_HEADER_BYTES + 128 * 1024;
/// Large, file-dense repositories can take longer than the provider's ordinary
/// lifecycle deadline to enumerate, compress, and stream. The response remains
/// byte-bounded, and periodic chat captures are preempted before publication
/// whenever new input arrives.
const WORKSPACE_CHECKPOINT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
const RUNTIME_POLICY_VERSION: u64 = 2;
const RUNTIME_GATEWAY_PROBE_VERSION: u64 = 2;

/// Where a pod says what it is and what it can take.
const READY_PATH: &str = "/readyz";

/// How long to wait for a freshly created sandbox to start serving.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How often to re-check a sandbox that is not yet running.
const READY_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Exact-token recovery scheduled when a hold request future is cancelled.
/// The guard is armed only after readiness and request validation have passed,
/// immediately before the runtime can observe the hold request.
struct HeldCheckpointRecoveryGuard {
    client: SandboxClient,
    endpoint: String,
    resume_token: String,
    armed: bool,
}

impl HeldCheckpointRecoveryGuard {
    fn new(client: SandboxClient, endpoint: String, resume_token: String) -> Self {
        Self {
            client,
            endpoint,
            resume_token,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    async fn recover(&mut self) -> Result<(), AdapterError> {
        match resume_workspace_writers_with_client(&self.client, &self.endpoint, &self.resume_token)
            .await
        {
            Ok(()) => {
                self.disarm();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for HeldCheckpointRecoveryGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!("held workspace checkpoint recovery could not be scheduled");
            return;
        };
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let resume_token = self.resume_token.clone();
        runtime.spawn(async move {
            if let Err(error) =
                resume_workspace_writers_with_client(&client, &endpoint, &resume_token).await
            {
                tracing::error!(%error, "cancelled held workspace checkpoint could not be recovered");
            }
        });
    }
}

#[derive(serde::Deserialize)]
struct RuntimeReadiness {
    status: String,
    #[serde(default, deserialize_with = "deserialize_prelude_support")]
    prelude: Option<bool>,
}

fn deserialize_prelude_support<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <bool as serde::Deserialize>::deserialize(deserializer).map(Some)
}

#[derive(serde::Serialize)]
struct PreludeHeader {
    #[serde(rename = "chronoPrelude")]
    format: u8,
    files: Vec<PreludeEntry>,
}

#[derive(serde::Serialize)]
struct PreludeEntry {
    path: String,
    len: usize,
}

struct BoundedPrelude {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl BoundedPrelude {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            exceeded: false,
        }
    }
}

impl Write for BoundedPrelude {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > PRELUDE_LIMIT)
        {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "runtime prelude limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn oversized_prelude() -> AdapterError {
    AdapterError::Unsupported(format!(
        "this session's runtime prelude exceeds the {PRELUDE_LIMIT}-byte limit"
    ))
}

fn add_prelude_entry(
    entries: &mut Vec<PreludeEntry>,
    path_bytes: &mut usize,
    path: &str,
    len: usize,
) -> Result<(), AdapterError> {
    if !is_canonical_prelude_path(path) {
        return Err(AdapterError::Unsupported(
            "runtime prelude contains an unsafe, noncanonical, or overlong path".into(),
        ));
    }
    *path_bytes = path_bytes
        .checked_add(path.len())
        .filter(|bytes| *bytes <= PRELUDE_LIMIT)
        .ok_or_else(oversized_prelude)?;
    entries.push(PreludeEntry {
        path: path.to_owned(),
        len,
    });
    Ok(())
}

fn validate_prelude_targets(entries: &[PreludeEntry]) -> Result<(), AdapterError> {
    // Component ordering places every file immediately before its first
    // descendant. Keep only references here: copying every prefix of a deep
    // path would turn the bounded wire header into quadratic allocation.
    let mut targets = entries
        .iter()
        .map(|entry| Path::new(entry.path.as_str()))
        .collect::<Vec<_>>();
    targets.sort_unstable_by(|left, right| left.components().cmp(right.components()));
    if targets
        .windows(2)
        .any(|pair| is_component_prefix(pair[0], pair[1]))
    {
        return Err(AdapterError::Unsupported(
            "runtime prelude contains conflicting file and directory targets".into(),
        ));
    }
    Ok(())
}

fn is_component_prefix(prefix: &Path, path: &Path) -> bool {
    let mut components = path.components();
    prefix
        .components()
        .all(|component| components.next() == Some(component))
}

fn is_canonical_prelude_path(raw: &str) -> bool {
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return false;
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return false;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            return false;
        };
        if part.len() > PRELUDE_COMPONENT_LIMIT {
            return false;
        }
        normalized.push(part);
    }
    !normalized.as_os_str().is_empty() && normalized.as_os_str() == path.as_os_str()
}

impl From<SandboxError> for AdapterError {
    fn from(e: SandboxError) -> Self {
        match e {
            // A runtime answering "not implemented" is stating an absence, not
            // reporting a failure. Read as a rejection it would look like
            // something going wrong every time rotation asked a runtime that
            // was never able to answer.
            SandboxError::Rejected { status: 501, .. } => AdapterError::Unsupported(
                "this runtime declares no support for what was asked".to_string(),
            ),
            /*
             * The provider saying a sandbox is not there is the clearest
             * statement of loss there is, and it arrived as a rejection —
             * which now means "something answered and refused". Read that way
             * a deleted sandbox would never be replaced, so the statuses that
             * mean absence are separated from the ones that mean refusal.
             *
             * 404 and 410 are the provider's answer about this sandbox. 5xx is
             * the provider failing to answer about anything, which says
             * nothing about the sandbox either way — and treating "the
             * provider is briefly broken" as "the sandbox is gone" would
             * replace a healthy runtime, so it stays out of loss.
             */
            SandboxError::Rejected {
                status: status @ (404 | 410),
                detail,
            } => AdapterError::Unavailable(format!("this sandbox is gone ({status}: {detail})")),
            /*
             * The production probe for issue #551 on 2026-08-23 received no
             * response to create before its 240-second client deadline. A
             * separate observer saw only Pending / BATCHSANDBOX_PENDING, the
             * provider's generic allocation state. Neither is an exact
             * capacity signal, so this adapter deliberately does not map them
             * to AtCapacity. A future provider contract may add a specific
             * response here; arbitrary status prose must never be guessed.
             */
            SandboxError::Rejected { status, detail } => {
                AdapterError::Rejected(format!("{status}: {detail}"))
            }
            SandboxError::IdentityMismatch(detail) => AdapterError::Rejected(detail),
            SandboxError::Unsupported(detail) => AdapterError::Unsupported(detail),
            other => AdapterError::Unavailable(other.to_string()),
        }
    }
}

/// Classify a response from a live supervisor rather than a provider lookup.
///
/// A 404 from the provider means the sandbox is gone; a 404 from a route on a
/// supervisor we reached means that supervisor answered and refused. Keeping
/// those contexts separate prevents an invalid tree name or unknown capture id
/// from triggering runtime-loss recovery.
fn runtime_operation_error(error: SandboxError) -> AdapterError {
    match error {
        SandboxError::Unreachable(detail) => AdapterError::Unavailable(detail),
        SandboxError::Rejected {
            status: 501,
            detail,
        }
        | SandboxError::Unsupported(detail) => AdapterError::Unsupported(detail),
        SandboxError::Rejected { status, detail } => {
            AdapterError::Rejected(format!("{status}: {detail}"))
        }
        SandboxError::Malformed(detail) => AdapterError::Rejected(detail),
        SandboxError::IdentityMismatch(detail) => AdapterError::Rejected(detail),
        SandboxError::Incomplete { pages, page_size } => AdapterError::Rejected(format!(
            "sandbox listing did not end after {pages} pages of {page_size}"
        )),
    }
}

/// How sandboxes are shaped, whichever kernel is in them.
#[derive(Debug, Clone)]
pub struct SandboxShape {
    /// Image carrying the kernel and the supervisor.
    pub image: String,
    /// Entrypoint that starts the supervisor.
    pub entrypoint: Vec<String>,
    /// Hard resource caps for each sandbox.
    pub resource_limits: BTreeMap<String, String>,
    /// Seconds before the provider reclaims the sandbox, or `None` for never.
    ///
    /// `None` is the default and is deliberate. A session is a person's working
    /// terminal: expiring it on a timer ends live work mid-turn, and no timer
    /// can tell an idle session from one whose owner is reading. Teardown is an
    /// explicit act by the owner, and the reconciliation sweep is what reclaims
    /// sandboxes whose session is genuinely gone.
    pub timeout_seconds: Option<u64>,
    /// How often to ask a runtime for a reading, or `None` never to ask.
    ///
    /// Asked at attach rather than baked into the image, so changing cadence
    /// does not rotate and interrupt every live runtime.
    pub telemetry_interval_ms: Option<u64>,
    /// Directory the agent works in, inside its sandbox.
    pub workspace: String,
}

impl SandboxShape {
    /// What the provider is asked to reserve for one sandbox.
    ///
    /// The only place in this service where a provider's quantity syntax is
    /// written. The core holds numbers and this derives the strings, so
    /// nothing ever parses one back -- which is what lets the figure a session
    /// was quoted and the figure it was provisioned at be the same object
    /// rather than two things kept in step.
    ///
    /// A stated capacity replaces the shape's map **whole**, never key by
    /// key. Merging would let a session run at its own memory and the
    /// deployment's CPU, which is a shape nobody asked for; and it would let
    /// any other key a deployment ever puts in the shape survive beside a
    /// per-session figure. Stating a capacity is stating both figures,
    /// because `RuntimeCapacity` cannot express half of one.
    ///
    /// CPU is rendered in millicores rather than reduced to whole cores:
    /// `1500m` needs no division and has no remainder case, and the provider
    /// reads `1000m` and `1` as the same reservation.
    pub(crate) fn limits_for(&self, capacity: Option<RuntimeCapacity>) -> BTreeMap<String, String> {
        match capacity {
            Some(capacity) => BTreeMap::from([
                ("cpu".to_string(), format!("{}m", capacity.cpu_millis)),
                ("memory".to_string(), format!("{}Mi", capacity.memory_mib)),
            ]),
            None => self.resource_limits.clone(),
        }
    }

    fn timeout_for(&self, requested: Option<u64>) -> Option<u64> {
        requested.or(self.timeout_seconds)
    }
}

impl Default for SandboxShape {
    fn default() -> Self {
        Self {
            image: "cma-runtime:dev".into(),
            entrypoint: vec!["/usr/local/bin/layer4-runtime".into()],
            resource_limits: BTreeMap::from([
                ("cpu".to_string(), "1".to_string()),
                ("memory".to_string(), "1Gi".to_string()),
            ]),
            timeout_seconds: None,
            // Slow enough not to dominate an idle session's traffic, while
            // bounding the last reading before a kill to two seconds of age.
            telemetry_interval_ms: Some(2_000),
            workspace: "/home/chrono/workspace".into(),
        }
    }
}

/// Who is waiting to hear whether a plane switch happened, by runtime handle.
///
/// Shared rather than owned: a link's inbound task is what hears the answer,
/// and it outlives the call that asked the question.
#[derive(Clone, Default)]
pub(crate) struct SwitchWaiters(std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, SwitchWaiter>>>);

struct SwitchWaiter {
    link: WeakTerminalInputSender,
    answer: oneshot::Sender<bool>,
}

impl SwitchWaiters {
    /// Register interest in the next answer from this link generation.
    pub(crate) async fn expect(
        &self,
        handle: &str,
        link: &TerminalInputSender,
    ) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        // A second switch replaces the first waiter; the earlier caller sees
        // its sender dropped and rotates, which is the safe reading.
        self.0.lock().await.insert(
            handle.to_string(),
            SwitchWaiter {
                // Identity only. Keeping a strong sender here would keep the
                // control lane alive after every real owner had dropped it.
                link: link.downgrade(),
                answer: tx,
            },
        );
        rx
    }

    /// Hand an answer to whoever asked on this same link generation.
    ///
    /// An answer nobody awaits is dropped. In particular, an old socket may
    /// answer after a replacement has registered its own waiter, and that old
    /// answer says nothing about the command sent through the replacement.
    pub(crate) async fn settle(&self, handle: &str, link: &TerminalInputSender, accepted: bool) {
        let mut waiters = self.0.lock().await;
        let is_same_link = waiters
            .get(handle)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if !is_same_link {
            return;
        }
        if let Some(waiter) = waiters.remove(handle) {
            let _ = waiter.answer.send(accepted);
        }
    }

    /// Forget a waiter only if it belongs to the link that has gone.
    pub(crate) async fn forget(&self, handle: &str, link: &TerminalInputSender) {
        let mut waiters = self.0.lock().await;
        let is_same_link = waiters
            .get(handle)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if is_same_link {
            waiters.remove(handle);
        }
    }
}

#[derive(Clone)]
struct RegisteredLink {
    sender: TerminalInputSender,
    retired: TaskRetirement,
}

impl RegisteredLink {
    async fn retire(&self) {
        self.sender.retire().await;
        self.retired.wait().await;
    }
}

/// Abort a generation when opening it is canceled or returns an error.
pub(crate) struct AbortLinkOnDrop {
    link: TerminalInputSender,
    armed: bool,
}

impl AbortLinkOnDrop {
    pub(crate) fn new(link: TerminalInputSender) -> Self {
        Self { link, armed: true }
    }

    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AbortLinkOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.link.abort();
        }
    }
}

/// Credential-install acknowledgements, correlated to one request and socket generation.
#[derive(Clone, Default)]
pub(crate) struct CredentialWaiters(
    std::sync::Arc<tokio::sync::Mutex<BTreeMap<(String, String), CredentialWaiter>>>,
);

struct CredentialWaiter {
    link: WeakTerminalInputSender,
    answer: oneshot::Sender<bool>,
}

impl CredentialWaiters {
    /// Register before enqueueing so even an immediate reply has an owner.
    pub(crate) async fn expect(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
    ) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.0.lock().await.insert(
            (handle.to_string(), operation_id.to_string()),
            CredentialWaiter {
                link: link.downgrade(),
                answer: tx,
            },
        );
        rx
    }

    /// Settle only the exact operation sent through this exact socket generation.
    pub(crate) async fn settle(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
        installed: bool,
    ) {
        let key = (handle.to_string(), operation_id.to_string());
        let mut waiters = self.0.lock().await;
        let is_same_link = waiters
            .get(&key)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if !is_same_link {
            return;
        }
        if let Some(waiter) = waiters.remove(&key) {
            let _ = waiter.answer.send(installed);
        }
    }

    /// Remove one timed-out or failed operation without disturbing a successor.
    pub(crate) async fn forget_operation(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
    ) {
        let key = (handle.to_string(), operation_id.to_string());
        let mut waiters = self.0.lock().await;
        let is_same_link = waiters
            .get(&key)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if is_same_link {
            waiters.remove(&key);
        }
    }

    /// Fail an operation whose frame could not be written to its socket.
    pub(crate) async fn fail(&self, handle: &str, operation_id: &str) {
        let waiter = self
            .0
            .lock()
            .await
            .remove(&(handle.to_string(), operation_id.to_string()));
        if let Some(waiter) = waiter {
            let _ = waiter.answer.send(false);
        }
    }

    /// Cancel every request belonging to a socket that closed or was replaced.
    pub(crate) async fn forget_link(&self, handle: &str, link: &TerminalInputSender) {
        let mut waiters = self.0.lock().await;
        waiters.retain(|(runtime, _), waiter| {
            runtime != handle
                || waiter
                    .link
                    .upgrade()
                    .is_some_and(|expected| !expected.same_channel(link))
        });
    }
}

/// Runtime-policy acknowledgements, correlated to one operation and socket generation.
#[derive(Clone, Default)]
pub(crate) struct RuntimePolicyWaiters(
    std::sync::Arc<tokio::sync::Mutex<BTreeMap<(String, String), RuntimePolicyWaiter>>>,
);

struct RuntimePolicyWaiter {
    link: WeakTerminalInputSender,
    answer: oneshot::Sender<link::RuntimePolicyAnswer>,
}

impl RuntimePolicyWaiters {
    pub(crate) async fn expect(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
    ) -> oneshot::Receiver<link::RuntimePolicyAnswer> {
        let (tx, rx) = oneshot::channel();
        self.0.lock().await.insert(
            (handle.to_string(), operation_id.to_string()),
            RuntimePolicyWaiter {
                link: link.downgrade(),
                answer: tx,
            },
        );
        rx
    }

    pub(crate) async fn settle(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
        answer: link::RuntimePolicyAnswer,
    ) {
        let key = (handle.to_string(), operation_id.to_string());
        let mut waiters = self.0.lock().await;
        let same_link = waiters
            .get(&key)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if !same_link {
            return;
        }
        if let Some(waiter) = waiters.remove(&key) {
            let _ = waiter.answer.send(answer);
        }
    }

    pub(crate) async fn forget_operation(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
    ) {
        let key = (handle.to_string(), operation_id.to_string());
        let mut waiters = self.0.lock().await;
        let same_link = waiters
            .get(&key)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if same_link {
            waiters.remove(&key);
        }
    }

    pub(crate) async fn fail(&self, handle: &str, operation_id: &str) {
        self.0
            .lock()
            .await
            .remove(&(handle.to_string(), operation_id.to_string()));
    }

    pub(crate) async fn forget_link(&self, handle: &str, link: &TerminalInputSender) {
        self.0.lock().await.retain(|(runtime, _), waiter| {
            runtime != handle
                || waiter
                    .link
                    .upgrade()
                    .is_some_and(|expected| !expected.same_channel(link))
        });
    }
}

/// Sponsored-model authority acknowledgements, correlated to one operation and
/// one exact socket generation.
#[derive(Clone, Default)]
pub(crate) struct SponsoredModelAuthorityWaiters(
    std::sync::Arc<tokio::sync::Mutex<BTreeMap<(String, String), SponsoredModelAuthorityWaiter>>>,
);

struct SponsoredModelAuthorityWaiter {
    link: WeakTerminalInputSender,
    answer: oneshot::Sender<link::SponsoredModelAuthorityAnswer>,
}

impl SponsoredModelAuthorityWaiters {
    pub(crate) async fn expect(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
    ) -> oneshot::Receiver<link::SponsoredModelAuthorityAnswer> {
        let (tx, rx) = oneshot::channel();
        self.0.lock().await.insert(
            (handle.to_string(), operation_id.to_string()),
            SponsoredModelAuthorityWaiter {
                link: link.downgrade(),
                answer: tx,
            },
        );
        rx
    }

    pub(crate) async fn settle(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
        answer: link::SponsoredModelAuthorityAnswer,
    ) {
        let key = (handle.to_string(), operation_id.to_string());
        let mut waiters = self.0.lock().await;
        let same_link = waiters
            .get(&key)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if same_link {
            if let Some(waiter) = waiters.remove(&key) {
                let _ = waiter.answer.send(answer);
            }
        }
    }

    pub(crate) async fn forget_operation(
        &self,
        handle: &str,
        operation_id: &str,
        link: &TerminalInputSender,
    ) {
        let key = (handle.to_string(), operation_id.to_string());
        let mut waiters = self.0.lock().await;
        let same_link = waiters
            .get(&key)
            .and_then(|waiter| waiter.link.upgrade())
            .is_some_and(|expected| expected.same_channel(link));
        if same_link {
            waiters.remove(&key);
        }
    }

    pub(crate) async fn fail(&self, handle: &str, operation_id: &str) {
        self.0
            .lock()
            .await
            .remove(&(handle.to_string(), operation_id.to_string()));
    }

    pub(crate) async fn forget_link(&self, handle: &str, link: &TerminalInputSender) {
        self.0.lock().await.retain(|(runtime, _), waiter| {
            runtime != handle
                || waiter
                    .link
                    .upgrade()
                    .is_some_and(|expected| !expected.same_channel(link))
        });
    }
}

/// Registration and cleanup for one runtime link generation.
///
/// A generation supervisor owns reader, writer, and registry cleanup, while
/// callers use the shared map to deliver credentials and switches. Keeping the
/// completion receipt beside the sender makes replacement wait for that whole
/// supervisor and makes every close remove only its exact socket generation.
#[derive(Clone)]
pub(crate) struct LinkLifecycle {
    runtime: String,
    registry_key: String,
    links: std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, RegisteredLink>>>,
    switch_waiters: SwitchWaiters,
    credential_waiters: CredentialWaiters,
    runtime_policy_waiters: RuntimePolicyWaiters,
    sponsored_model_authority_waiters: SponsoredModelAuthorityWaiters,
}

fn link_key_belongs_to(key: &str, runtime: &str) -> bool {
    key == runtime
        || key
            .strip_prefix(runtime)
            .is_some_and(|suffix| suffix.starts_with('\0'))
}

fn registered_link_key(
    links: &BTreeMap<String, RegisteredLink>,
    runtime: &str,
    link: &TerminalInputSender,
) -> Option<String> {
    links.iter().find_map(|(key, current)| {
        (link_key_belongs_to(key, runtime) && current.sender.same_channel(link))
            .then(|| key.clone())
    })
}

fn runtime_control_link<'a>(
    links: &'a BTreeMap<String, RegisteredLink>,
    runtime: &str,
) -> Option<&'a RegisteredLink> {
    links
        .iter()
        .find(|(key, link)| link_key_belongs_to(key, runtime) && !link.sender.is_closed())
        .map(|(_, link)| link)
}

impl LinkLifecycle {
    /// Install a fully supervised generation and retire the one it displaced.
    pub(crate) async fn register(
        &self,
        link: TerminalInputSender,
        retired: TaskRetirement,
        installed: oneshot::Sender<()>,
    ) -> bool {
        let registration = RegisteredLink {
            sender: link.clone(),
            retired,
        };
        let previous = {
            let mut links = self.links.lock().await;
            if links
                .get(&self.registry_key)
                .is_some_and(|current| current.sender.same_channel(&link))
            {
                if link.is_closed() {
                    links.remove(&self.registry_key);
                    let _ = installed.send(());
                    return false;
                }
                let _ = installed.send(());
                return true;
            }
            let previous = links.insert(self.registry_key.clone(), registration);
            // There is no await between insertion and opening this barrier. If
            // the caller is canceled before insertion, dropping `installed`
            // tells the already-running supervisor there is nothing to forget.
            let _ = installed.send(());
            previous
        };
        if let Some(previous) = previous {
            // This cleanup is service-owned once the map swap commits. If the
            // opening caller is canceled while waiting, the displaced socket
            // must still retire instead of surviving unregistered.
            let runtime = self.runtime.clone();
            let switch_waiters = self.switch_waiters.clone();
            let credential_waiters = self.credential_waiters.clone();
            let runtime_policy_waiters = self.runtime_policy_waiters.clone();
            let sponsored_model_authority_waiters = self.sponsored_model_authority_waiters.clone();
            let retirement = tokio::spawn(async move {
                switch_waiters.forget(&runtime, &previous.sender).await;
                credential_waiters
                    .forget_link(&runtime, &previous.sender)
                    .await;
                runtime_policy_waiters
                    .forget_link(&runtime, &previous.sender)
                    .await;
                sponsored_model_authority_waiters
                    .forget_link(&runtime, &previous.sender)
                    .await;
                previous.retire().await;
            });
            let _ = retirement.await;
        }

        let mut links = self.links.lock().await;
        let is_current = links
            .get(&self.registry_key)
            .is_some_and(|current| current.sender.same_channel(&link));
        if is_current && link.is_closed() {
            links.remove(&self.registry_key);
            return false;
        }
        is_current
    }

    /// Forget this generation without disturbing a successor already present.
    pub(crate) async fn forget(&self, link: &TerminalInputSender) {
        let mut links = self.links.lock().await;
        if let Some(key) = registered_link_key(&links, &self.runtime, link) {
            links.remove(&key);
        }
        drop(links);
        // A successor may be in the map while this generation still owns the
        // previous waiter, so waiter cleanup independently checks identity.
        self.switch_waiters.forget(&self.runtime, link).await;
        self.credential_waiters
            .forget_link(&self.runtime, link)
            .await;
        self.runtime_policy_waiters
            .forget_link(&self.runtime, link)
            .await;
        self.sponsored_model_authority_waiters
            .forget_link(&self.runtime, link)
            .await;
    }

    pub(crate) async fn forget_if_whole_retired(&self, link: &TerminalInputSender) {
        let removed = {
            let mut links = self.links.lock().await;
            let key = links.iter().find_map(|(key, current)| {
                (link_key_belongs_to(key, &self.runtime)
                    && current.sender.same_channel(link)
                    && current.retired.is_retired())
                .then(|| key.clone())
            });
            key.and_then(|key| links.remove(&key))
        };
        if removed.is_some() {
            self.switch_waiters.forget(&self.runtime, link).await;
            self.credential_waiters
                .forget_link(&self.runtime, link)
                .await;
            self.runtime_policy_waiters
                .forget_link(&self.runtime, link)
                .await;
            self.sponsored_model_authority_waiters
                .forget_link(&self.runtime, link)
                .await;
        }
    }

    /// Settle a switch only when it was sent through this same generation.
    pub(crate) async fn settle(&self, link: &TerminalInputSender, accepted: bool) {
        self.switch_waiters
            .settle(&self.runtime, link, accepted)
            .await;
    }

    pub(crate) async fn is_current(&self, link: &TerminalInputSender) -> bool {
        let links = self.links.lock().await;
        registered_link_key(&links, &self.runtime, link).is_some() && !link.is_closed()
    }

    /// Settle a credential request only on the link generation that carried it.
    pub(crate) async fn settle_credential(
        &self,
        link: &TerminalInputSender,
        operation_id: &str,
        installed: bool,
    ) {
        self.credential_waiters
            .settle(&self.runtime, operation_id, link, installed)
            .await;
    }

    /// Fail an exact operation after this link's WebSocket write failed.
    pub(crate) async fn fail_credential(&self, operation_id: &str) {
        self.credential_waiters
            .fail(&self.runtime, operation_id)
            .await;
    }

    pub(crate) async fn settle_runtime_policy(
        &self,
        link: &TerminalInputSender,
        operation_id: &str,
        answer: link::RuntimePolicyAnswer,
    ) {
        self.runtime_policy_waiters
            .settle(&self.runtime, operation_id, link, answer)
            .await;
    }

    pub(crate) async fn fail_runtime_policy(&self, operation_id: &str) {
        self.runtime_policy_waiters
            .fail(&self.runtime, operation_id)
            .await;
    }

    pub(crate) async fn settle_sponsored_model_authority(
        &self,
        link: &TerminalInputSender,
        operation_id: &str,
        answer: link::SponsoredModelAuthorityAnswer,
    ) {
        self.sponsored_model_authority_waiters
            .settle(&self.runtime, operation_id, link, answer)
            .await;
    }

    pub(crate) async fn fail_sponsored_model_authority(&self, operation_id: &str) {
        self.sponsored_model_authority_waiters
            .fail(&self.runtime, operation_id)
            .await;
    }
}

/// A kernel running in a provider-managed sandbox.
pub struct HostedKernel {
    pub(crate) client: SandboxClient,
    shape: SandboxShape,
    /// Which kernel this is, recorded on the sandboxes it creates.
    kernel: &'static str,
    /// Whether this lane refuses runtimes without exact canonical kernel
    /// metadata. New dedicated lanes set this; ordinary lanes retain only the
    /// explicitly documented pre-metadata compatibility behavior.
    strict_kernel_metadata: bool,
    /// Where this kernel reads its skills, relative to the pod's home.
    ///
    /// Held so paths are composed here rather than by anything above: the pod
    /// is handed paths and writes them, and the core names skills and never
    /// where they live.
    skills_root: String,
    /// Where this kernel reads standing instructions, relative to the home.
    agent_notes: String,
    /// The open link to each runtime, by handle.
    ///
    /// Held because a credential has to reach a runtime that is already
    /// running, and the supervisor accepts one only over a link that is
    /// already open. Opening a second attachment to deliver it is not an
    /// option: attaching declares a geometry, and a connection made without
    /// one would resize the pty under whoever is looking at it.
    links: std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, RegisteredLink>>>,
    /// Who is waiting to hear whether a plane switch was performed.
    ///
    /// A switch is only recorded once the runtime says it happened (#289):
    /// the frame reaching the socket says nothing about the program actually
    /// being swapped, and a supervisor too old to know the frame ignores it
    /// in silence. One waiter per runtime, because a session has one plane
    /// and cannot be moved twice at once.
    switch_waiters: SwitchWaiters,
    /// Credential acknowledgements awaiting an exact operation and link generation.
    credential_waiters: CredentialWaiters,
    /// Opaque runtime-policy acknowledgements awaiting this socket generation.
    runtime_policy_waiters: RuntimePolicyWaiters,
    sponsored_model_authority_waiters: SponsoredModelAuthorityWaiters,
}

impl HostedKernel {
    /// Host `kernel` in sandboxes of this shape, from this provider.
    pub fn new(
        client: SandboxClient,
        shape: SandboxShape,
        kernel: &'static str,
        skills_root: impl Into<String>,
        agent_notes: impl Into<String>,
    ) -> Self {
        Self::new_with_kernel_metadata(client, shape, kernel, skills_root, agent_notes, false)
    }

    /// Host a new isolated lane that accepts only its canonical kernel label.
    pub fn new_strict(
        client: SandboxClient,
        shape: SandboxShape,
        kernel: &'static str,
        skills_root: impl Into<String>,
        agent_notes: impl Into<String>,
    ) -> Self {
        Self::new_with_kernel_metadata(client, shape, kernel, skills_root, agent_notes, true)
    }

    fn new_with_kernel_metadata(
        client: SandboxClient,
        shape: SandboxShape,
        kernel: &'static str,
        skills_root: impl Into<String>,
        agent_notes: impl Into<String>,
        strict_kernel_metadata: bool,
    ) -> Self {
        Self {
            client,
            shape,
            kernel,
            strict_kernel_metadata,
            skills_root: skills_root.into(),
            agent_notes: agent_notes.into(),
            links: std::sync::Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            switch_waiters: SwitchWaiters::default(),
            credential_waiters: CredentialWaiters::default(),
            runtime_policy_waiters: RuntimePolicyWaiters::default(),
            sponsored_model_authority_waiters: SponsoredModelAuthorityWaiters::default(),
        }
    }

    fn owns_sandbox(&self, metadata: &BTreeMap<String, String>) -> bool {
        let canonical = metadata.get(KERNEL_METADATA_KEY).map(String::as_str);
        let legacy = metadata.get(LEGACY_KERNEL_METADATA_KEY).map(String::as_str);
        match (canonical, legacy) {
            (Some(canonical), Some(legacy)) => canonical == self.kernel && legacy == self.kernel,
            (Some(canonical), None) => canonical == self.kernel,
            (None, Some(legacy)) => !self.strict_kernel_metadata && legacy == self.kernel,
            (None, None) => !self.strict_kernel_metadata,
        }
    }

    /// Where the agent works, inside its sandbox.
    pub fn workspace(&self) -> &str {
        &self.shape.workspace
    }

    /// Open one hosted ACP conversation under the shared evidence contract.
    ///
    /// Kernel adapters provide only their mode, resume, and credential
    /// choices. Socket framing, ACP translation, link lifecycle, and the
    /// turn-evidence boundary remain one production path for every ACP kernel.
    pub(crate) async fn open_acp_conversation(
        &self,
        handle: &RuntimeHandle,
        binding: AcpConversationBinding,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> Result<ConversationChannel, AdapterError> {
        debug_assert_eq!(
            binding.evidence_contract(),
            ConversationEvidenceContract::AcpV1
        );
        let (stream, url) = self
            .connect_conversation(handle, binding.conversation_slot.as_deref())
            .await?;
        let lifecycle =
            self.conversation_link_lifecycle(&handle.0, binding.conversation_slot.as_deref());
        tracing::info!(
            sandbox = %handle.0,
            kernel = self.kernel,
            %url,
            "opening the sandbox conversation"
        );
        let (conversation, control) = acp::Bridge::open(
            stream,
            binding
                .working_directory
                .unwrap_or_else(|| self.workspace().to_string()),
            binding.mode,
            binding.resume_conversation,
            Some(lifecycle),
            binding.own_credential,
            updates,
        )
        .await?;
        // The bridge records this control generation before its socket reader
        // starts, so a close racing the end of the handshake cannot leave a
        // dead sender registered here.
        drop(control);
        Ok(conversation)
    }

    /// Bring a sandbox up with this environment, and wait until it serves.
    pub async fn provision(
        &self,
        spec: &RuntimeSpec,
        env: BTreeMap<String, String>,
    ) -> Result<RuntimeHandle, AdapterError> {
        if spec.generation.is_some() != spec.reservation_token.is_some() {
            return Err(AdapterError::Rejected(
                "coordinated provisioning requires both a runtime generation and its reservation token"
                    .into(),
            ));
        }
        if spec.timeout_seconds == Some(0) {
            return Err(AdapterError::Rejected(
                "runtime timeout must be positive".into(),
            ));
        }
        if spec.conversation_slots_required == Some(0) {
            return Err(AdapterError::Rejected(
                "required conversation slot count must be positive".into(),
            ));
        }
        // Build and validate the exact bytes before provider creation. Skill
        // archive bodies alone do not include rooted paths, the prelude header,
        // or standing notes, and a candidate created before this check would be
        // guaranteed to fail identically on every retry.
        let prelude = self.prelude_for(
            &spec.skills,
            &spec.platform_briefing,
            &spec.environment.agent_notes,
        )?;
        let resource_limits = self.shape.limits_for(spec.capacity);
        let request = SandboxRequest {
            image: ImageSpec {
                uri: self.shape.image.clone(),
            },
            entrypoint: self.shape.entrypoint.clone(),
            env,
            resource_limits: resource_limits.clone(),
            timeout: self.shape.timeout_for(spec.timeout_seconds),
            // Correlates a sandbox back to the session that owns it, so an
            // orphan can be found and reclaimed.
            metadata: provisioning_metadata(spec, self.kernel),
        };

        let sandbox = self.client.create(&request).await?;
        tracing::info!(
            session_id = %spec.session_id,
            sandbox = %sandbox.id,
            kernel = self.kernel,
            image = %self.shape.image,
            cpu = resource_limits.get("cpu").map(String::as_str).unwrap_or("-"),
            memory = resource_limits.get("memory").map(String::as_str).unwrap_or("-"),
            "sandbox provisioned"
        );
        let sandbox = self.await_running(&sandbox.id).await?;
        let chat_capabilities_required =
            spec.workspace_source_required || spec.conversation_slots_required.is_some();
        let capability_delete_target = if spec.runtime_policy_required || chat_capabilities_required
        {
            self.client.capture_delete_target(&sandbox.id).await?
        } else {
            None
        };

        if spec.runtime_policy_required || chat_capabilities_required {
            let capability_result = async {
                if spec.runtime_policy_required
                    && (!self.supports_runtime_policy(&sandbox.id).await?
                        || !self.supports_runtime_gateway_probe(&sandbox.id).await?)
                {
                    return Err(AdapterError::Unsupported(
                        "this runtime image does not advertise github_capability_v2 and runtime_gateway_probe_v2"
                            .into(),
                    ));
                }
                if chat_capabilities_required {
                    let endpoint = self.client.endpoint(&sandbox.id, SUPERVISOR_PORT).await?;
                    let (_, readiness) = self.runtime_readiness(&endpoint.endpoint).await?;
                    require_chat_runtime_capabilities(
                        &readiness,
                        spec.conversation_slots_required.unwrap_or(1),
                    )?;
                }
                Ok(())
            }
            .await;

            if let Err(probe_error) = capability_result {
                // No failed readiness candidate is left behind. The target was
                // captured before probing, so cleanup cannot drift onto a
                // later provider object that happens to reuse the same handle.
                if let Some(target) = capability_delete_target {
                    if let Err(cleanup_error) = self.client.delete_captured(target).await {
                        tracing::error!(
                            sandbox = %sandbox.id,
                            probe = %probe_error,
                            cleanup = %cleanup_error,
                            "failed readiness candidate could not be reclaimed"
                        );
                        return Err(AdapterError::Unavailable(format!(
                            "runtime {} failed required capability readiness ({probe_error}); exact candidate cleanup also failed: {cleanup_error}",
                            sandbox.id,
                        )));
                    }
                }
                return Err(probe_error);
            }
        }

        if let Some(workspace) = spec.workspace.as_ref() {
            self.hand_over_workspace(&sandbox.id, workspace).await?;
        }

        // Handed over before anything attaches, which is what makes this
        // possible at all: the supervisor does not start the agent until the
        // first attachment, so a conversation delivered now is in place before
        // the agent looks for one.
        if let Some(conversation) = spec.restore_from.as_ref() {
            self.hand_over(&sandbox.id, conversation).await?;
        }

        // Skills land in the same window and for the same reason. The agent
        // reads them when it starts, so delivering now is delivering before
        // anything can look — and a session whose skills arrive afterwards is
        // a session promised tools it is not using.
        // The skills and the note the agent reads before using them travel
        // together: both have to be on disk before the kernel starts, and both
        // are useless if they land after it has already decided what to do.
        if let Some(prelude) = prelude {
            self.install_prelude(&sandbox.id, &spec.skills, prelude)
                .await?;
        }

        Ok(RuntimeHandle(sandbox.id))
    }

    /// Compose two separately owned instruction sources without merging them.
    fn agent_note(&self, platform_briefing: &str, owner_notes: &str) -> String {
        let mut note = platform_briefing.to_string();
        /*
         * The owner's own instructions, after the platform's and separated.
         *
         * Appended rather than merged: the two sources have different owners.
         * An owner may contradict the platform in their prose, but they cannot
         * remove or rewrite what the platform placed before it.
         */
        let owner_notes = owner_notes.trim();
        if !owner_notes.is_empty() {
            note.truncate(note.trim_end().len());
            if !note.is_empty() {
                note.push_str("\n\n---\n\n");
            }
            note.push_str("# From the owner of this session\n\n");
            note.push_str(owner_notes);
            note.push('\n');
        }
        note
    }

    /// Encode exactly what the supervisor will receive, with its actual limit.
    fn prelude_for(
        &self,
        skills: &[SkillPackage],
        platform_briefing: &str,
        owner_notes: &str,
    ) -> Result<Option<Vec<u8>>, AdapterError> {
        let notes_path = self.agent_notes.trim();
        if skills.is_empty() && notes_path.is_empty() {
            return Ok(None);
        }
        let root = self.skills_root.trim();
        if !skills.is_empty() && root.is_empty() {
            return Err(AdapterError::Unsupported(
                "this runtime's agent reads no skills".to_string(),
            ));
        }

        let note = (!notes_path.is_empty())
            .then(|| self.agent_note(platform_briefing, owner_notes).into_bytes());
        let file_count = skills
            .iter()
            .map(|skill| skill.files.len())
            .try_fold(usize::from(note.is_some()), usize::checked_add)
            .ok_or_else(oversized_prelude)?;
        if file_count > PRELUDE_FILE_LIMIT {
            return Err(oversized_prelude());
        }
        let mut entries = Vec::with_capacity(file_count);
        let mut path_bytes = 0_usize;
        if let Some(body) = note.as_ref() {
            add_prelude_entry(&mut entries, &mut path_bytes, notes_path, body.len())?;
        }
        for skill in skills {
            for (relative, body) in &skill.files {
                root.len()
                    .checked_add(skill.name.len())
                    .and_then(|bytes| bytes.checked_add(relative.len()))
                    .and_then(|bytes| bytes.checked_add(2))
                    .filter(|bytes| *bytes <= PRELUDE_LIMIT)
                    .ok_or_else(oversized_prelude)?;
                let path = format!("{root}/{}/{relative}", skill.name);
                add_prelude_entry(&mut entries, &mut path_bytes, &path, body.len())?;
            }
        }
        validate_prelude_targets(&entries)?;

        let mut pack = BoundedPrelude::new();
        if let Err(error) = serde_json::to_writer(
            &mut pack,
            &PreludeHeader {
                format: 1,
                files: entries,
            },
        ) {
            return Err(if pack.exceeded {
                oversized_prelude()
            } else {
                AdapterError::Rejected(format!("runtime prelude header is invalid: {error}"))
            });
        }
        pack.write_all(b"\n").map_err(|_| oversized_prelude())?;
        if let Some(body) = note.as_ref() {
            pack.write_all(body).map_err(|_| oversized_prelude())?;
        }
        for skill in skills {
            for (_, body) in &skill.files {
                pack.write_all(body).map_err(|_| oversized_prelude())?;
            }
        }
        Ok(Some(pack.bytes))
    }

    /// Put a session's skills on its pod's disk, under this kernel's root.
    ///
    /// The paths are composed here, so the pod is handed paths and writes
    /// them and never learns what a skill is — the same naivety the
    /// conversation carry commits to, and for the same reason.
    ///
    /// A supervisor that cannot take a prelude is refused by name rather than
    /// having a POST land on a router without the route: that answers 404,
    /// which reads as a sandbox that is gone and would replace a healthy one.
    async fn install_prelude(
        &self,
        sandbox: &str,
        skills: &[SkillPackage],
        pack: Vec<u8>,
    ) -> Result<(), AdapterError> {
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        if !self.takes_a_prelude(&endpoint.endpoint).await? {
            return Err(AdapterError::Unsupported(format!(
                "this runtime's image cannot be given skills, so {} would be missing",
                skills
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let bytes = pack.len();
        self.client
            .post_to_sandbox(&endpoint.endpoint, PRELUDE_PATH, pack)
            .await?;
        tracing::info!(
            %sandbox,
            skills = skills.len(),
            bytes,
            "wrote this session's skills and its standing instructions"
        );
        Ok(())
    }

    /// Whether this pod's supervisor can be handed files at all.
    ///
    /// Read from readiness, which every image ever built answers. An older one
    /// answers without the key, which is unambiguous — and asking is cheaper
    /// than discovering it from a 404 that means something else entirely.
    async fn takes_a_prelude(&self, endpoint: &str) -> Result<bool, AdapterError> {
        let (readiness, _) = self.runtime_readiness(endpoint).await?;
        Ok(readiness.prelude.unwrap_or(false))
    }

    async fn supports_runtime_policy(&self, sandbox: &str) -> Result<bool, AdapterError> {
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        let (_, body) = self.runtime_readiness(&endpoint.endpoint).await?;
        Ok(readiness_supports_runtime_policy(&body))
    }

    /// Read the exact non-secret policy tuple from one live supervisor.
    pub async fn observe_runtime_policy(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<crate::RuntimePolicyObservation, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let (_, body) = self.runtime_readiness(&endpoint.endpoint).await?;
        parse_runtime_policy_observation(&body)
    }

    async fn supports_runtime_gateway_probe(&self, sandbox: &str) -> Result<bool, AdapterError> {
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        let (_, body) = self.runtime_readiness(&endpoint.endpoint).await?;
        Ok(readiness_supports_runtime_gateway_probe(&body))
    }

    async fn runtime_readiness(
        &self,
        endpoint: &str,
    ) -> Result<(RuntimeReadiness, Vec<u8>), AdapterError> {
        let body = self
            .client
            .get_from_sandbox(endpoint, READY_PATH)
            .await?
            .ok_or_else(|| {
                SandboxError::Malformed("runtime readiness returned no document".into())
            })?;
        let readiness: RuntimeReadiness = serde_json::from_slice(&body).map_err(|error| {
            SandboxError::Malformed(format!("runtime readiness document: {error}"))
        })?;
        if readiness.status != "ready" {
            return Err(SandboxError::Malformed(format!(
                "runtime readiness reported status {:?}",
                readiness.status
            ))
            .into());
        };
        Ok((readiness, body))
    }

    async fn hand_over_workspace(
        &self,
        sandbox: &str,
        workspace: &WorkspacePackage,
    ) -> Result<(), AdapterError> {
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        let ready = self
            .client
            .get_from_sandbox(&endpoint.endpoint, READY_PATH)
            .await?
            .unwrap_or_default();
        let supported = serde_json::from_slice::<serde_json::Value>(&ready)
            .ok()
            .and_then(|value| {
                value
                    .get("workspaceCheckpoint")
                    .and_then(|capability| capability.get("version"))
                    .and_then(serde_json::Value::as_u64)
            })
            .is_some_and(|version| version >= WORKSPACE_VERSION);
        if !supported {
            return Err(AdapterError::Unsupported(
                "this runtime image does not advertise coherent workspace checkpoint v2".into(),
            ));
        }
        if workspace.epoch == 0
            || workspace.source_revision.len() != 40
            || workspace.baseline_manifest_digest.len() != 64
            || workspace.manifest_digest.len() != 64
            || workspace.archive_sha256.len() != 64
        {
            return Err(AdapterError::Rejected(
                "workspace handover metadata is invalid".into(),
            ));
        }
        let path = format!(
            "{WORKSPACE_PATH}?epoch={}&source={}&baseline={}&manifest={}&archive={}",
            workspace.epoch,
            workspace.source_revision,
            workspace.baseline_manifest_digest,
            workspace.manifest_digest,
            workspace.archive_sha256
        );
        let acknowledgement = self
            .client
            .post_to_sandbox_with_response(&endpoint.endpoint, &path, workspace.archive.clone())
            .await?;
        let acknowledgement: WorkspaceAcknowledgement = serde_json::from_slice(&acknowledgement)
            .map_err(|_| {
                AdapterError::Rejected("runtime did not acknowledge the restored workspace".into())
            })?;
        if acknowledgement.workspace_epoch != workspace.epoch
            || acknowledgement.source_revision != workspace.source_revision
            || acknowledgement.baseline_manifest_digest != workspace.baseline_manifest_digest
            || acknowledgement.manifest_digest != workspace.manifest_digest
        {
            return Err(AdapterError::Rejected(
                "runtime acknowledged a different workspace".into(),
            ));
        }
        tracing::info!(
            %sandbox,
            epoch = workspace.epoch,
            files = acknowledgement.entries,
            bytes = acknowledgement.bytes,
            "verified this session's repository workspace"
        );
        Ok(())
    }

    /// Prepare one Git source before this runtime starts any agent process.
    pub async fn prepare_workspace_source(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceSourceRequest,
    ) -> Result<crate::WorkspaceSourceReceipt, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let body = serde_json::to_vec(request)
            .expect("a workspace source request containing strings is serializable");
        let response = self
            .client
            .post_json_to_sandbox_with_response(&endpoint.endpoint, WORKSPACE_SOURCE_PATH, body)
            .await
            .map_err(runtime_operation_error)?;
        serde_json::from_slice(&response).map_err(|_| {
            AdapterError::Rejected("runtime returned an invalid workspace source receipt".into())
        })
    }

    /// Add one conversation worktree and return the runtime's canonical path.
    pub async fn add_workspace_tree(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceTreeRequest,
    ) -> Result<WorkspaceTreeReceipt, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let body = serde_json::to_vec(request)
            .expect("a workspace tree request containing strings is serializable");
        let response = self
            .client
            .post_json_to_sandbox_with_response(&endpoint.endpoint, WORKSPACE_TREES_PATH, body)
            .await
            .map_err(runtime_operation_error)?;
        serde_json::from_slice(&response).map_err(|_| {
            AdapterError::Rejected("runtime returned an invalid workspace tree receipt".into())
        })
    }

    /// Remove one exact named conversation worktree.
    pub async fn remove_workspace_tree(
        &self,
        handle: &RuntimeHandle,
        name: &str,
    ) -> Result<(), AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let name = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>();
        let path = format!("{WORKSPACE_TREES_PATH}/{name}");
        self.client
            .delete_from_sandbox(&endpoint.endpoint, &path)
            .await
            .map_err(runtime_operation_error)
    }

    /// Run administrator-owned setup through the supervisor's bounded runner.
    pub async fn run_workspace_setup(
        &self,
        handle: &RuntimeHandle,
        commands: &[String],
    ) -> Result<WorkspaceSetupReceipt, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let body = serde_json::to_vec(&WorkspaceSetupRequest { commands })
            .expect("a workspace setup request containing strings is serializable");
        let response = self
            .client
            .post_json_to_sandbox_with_response(&endpoint.endpoint, WORKSPACE_SETUP_PATH, body)
            .await
            .map_err(runtime_operation_error)?;
        let receipt: WorkspaceSetupReceipt = serde_json::from_slice(&response).map_err(|_| {
            AdapterError::Rejected("runtime returned an invalid workspace setup receipt".into())
        })?;
        receipt.validate(commands.len()).map_err(|_| {
            AdapterError::Rejected("runtime returned an invalid workspace setup receipt".into())
        })?;
        Ok(receipt)
    }

    /// Ask the shared Layer 4 supervisor for one coherent, resumable capture.
    pub async fn checkpoint_workspace(
        &self,
        handle: &RuntimeHandle,
        request: &WorkspaceCheckpointRequest,
    ) -> Result<WorkspaceCheckpointPackage, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let ready = self
            .client
            .get_from_sandbox(&endpoint.endpoint, READY_PATH)
            .await?
            .unwrap_or_default();
        if !readiness_supports_workspace_checkpoint(
            &ready,
            request.disposition,
            request.include_git_metadata,
        ) {
            return Err(AdapterError::Unsupported(
                "this runtime image does not advertise the requested workspace checkpoint capability"
                    .into(),
            ));
        }
        if request.epoch == 0
            || request.version == 0
            || !is_hex(&request.source_revision, 40)
            || !is_hex(&request.baseline_manifest_digest, 64)
        {
            return Err(AdapterError::Rejected(
                "workspace checkpoint request metadata is invalid".into(),
            ));
        }
        let disposition = match request.disposition {
            WorkspaceCheckpointDisposition::ResumeAfterCapture => "resume_after_capture",
            WorkspaceCheckpointDisposition::HoldForTeardown => "hold_for_teardown",
        };
        let resume_token = (request.disposition == WorkspaceCheckpointDisposition::HoldForTeardown)
            .then(workspace_writer_resume_token);
        let mut path = format!(
            "{WORKSPACE_CHECKPOINT_PATH}?epoch={}&version={}&source={}&baseline={}&include_git_metadata={}&disposition={disposition}",
            request.epoch,
            request.version,
            request.source_revision,
            request.baseline_manifest_digest,
            request.include_git_metadata,
        );
        if let Some(resume_token) = resume_token.as_deref() {
            path.push_str("&resume_token=");
            path.push_str(resume_token);
        }
        let mut recovery = resume_token.as_ref().map(|resume_token| {
            HeldCheckpointRecoveryGuard::new(
                self.client.clone(),
                endpoint.endpoint.clone(),
                resume_token.clone(),
            )
        });
        let wire = match self
            .client
            .post_to_sandbox_with_bounded_response_until(
                &endpoint.endpoint,
                &path,
                Vec::new(),
                MAX_WORKSPACE_RESPONSE_BYTES,
                WORKSPACE_CHECKPOINT_TIMEOUT,
            )
            .await
        {
            Ok(wire) => wire,
            Err(error) => {
                let original = AdapterError::from(error);
                if let Some(recovery) = recovery.as_mut() {
                    if recovery.recover().await.is_err() {
                        return Err(AdapterError::Unavailable(
                            HELD_CHECKPOINT_RECOVERY_ERROR.into(),
                        ));
                    }
                }
                return Err(original);
            }
        };
        let checkpoint = match parse_workspace_checkpoint(wire, request) {
            Ok(checkpoint) => checkpoint,
            Err(original @ AdapterError::Rejected(_))
                if request.disposition == WorkspaceCheckpointDisposition::HoldForTeardown =>
            {
                if recovery
                    .as_mut()
                    .expect("a held checkpoint has a recovery guard")
                    .recover()
                    .await
                    .is_err()
                {
                    return Err(AdapterError::Unavailable(
                        HELD_CHECKPOINT_RECOVERY_ERROR.into(),
                    ));
                }
                return Err(original);
            }
            Err(error) => return Err(error),
        };
        if let Some(expected) = resume_token.as_deref() {
            if checkpoint.writer_resume_token.as_deref() != Some(expected) {
                if recovery
                    .as_mut()
                    .expect("a held checkpoint has a recovery guard")
                    .recover()
                    .await
                    .is_err()
                {
                    return Err(AdapterError::Unavailable(
                        HELD_CHECKPOINT_RECOVERY_ERROR.into(),
                    ));
                }
                return Err(AdapterError::Rejected(
                    "runtime did not echo the caller-issued writer resume token".into(),
                ));
            }
            recovery
                .as_mut()
                .expect("a held checkpoint has a recovery guard")
                .disarm();
        }
        tracing::info!(
            sandbox = %handle.0,
            epoch = checkpoint.epoch,
            version = checkpoint.version,
            files = checkpoint.entries.len(),
            bytes = checkpoint.total_bytes,
            writers = checkpoint.quiesced_writers,
            "verified a coherent repository workspace checkpoint"
        );
        Ok(checkpoint)
    }

    pub async fn resume_workspace_writers(
        &self,
        handle: &RuntimeHandle,
        resume_token: &str,
    ) -> Result<(), AdapterError> {
        if resume_token.len() != 64 || !resume_token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AdapterError::Rejected(
                "workspace writer resume token is invalid".into(),
            ));
        }
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let ready = self
            .client
            .get_from_sandbox(&endpoint.endpoint, READY_PATH)
            .await?
            .unwrap_or_default();
        if !readiness_supports_workspace_checkpoint(
            &ready,
            WorkspaceCheckpointDisposition::HoldForTeardown,
            false,
        ) {
            return Err(AdapterError::Unsupported(
                "this runtime image cannot resume a held workspace checkpoint".into(),
            ));
        }
        self.resume_workspace_writers_at(&endpoint.endpoint, resume_token)
            .await
    }

    async fn resume_workspace_writers_at(
        &self,
        endpoint: &str,
        resume_token: &str,
    ) -> Result<(), AdapterError> {
        resume_workspace_writers_with_client(&self.client, endpoint, resume_token).await
    }

    /// Cancel candidate construction before a foreground worktree waits behind
    /// the runtime's checkpoint gate. A false receipt means no candidate was
    /// active and is still a successful, race-safe answer.
    pub async fn preempt_workspace_checkpoint(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<bool, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let ready = self
            .client
            .get_from_sandbox(&endpoint.endpoint, READY_PATH)
            .await?
            .unwrap_or_default();
        if workspace_checkpoint_version(&ready)
            .is_none_or(|version| version < WORKSPACE_PREEMPT_VERSION)
        {
            return Err(AdapterError::Unsupported(
                "this runtime image cannot preempt an in-flight workspace checkpoint".into(),
            ));
        }
        let response = self
            .client
            .post_to_sandbox_with_response(
                &endpoint.endpoint,
                WORKSPACE_CHECKPOINT_PREEMPT_PATH,
                Vec::new(),
            )
            .await?;
        let receipt: WorkspaceCheckpointPreemptReceipt = serde_json::from_slice(&response)
            .map_err(|_| {
                AdapterError::Rejected(
                    "runtime returned an invalid checkpoint preemption receipt".into(),
                )
            })?;
        Ok(receipt.cancellation_requested)
    }

    /// Run one closed read through the exact runtime's command sandbox and
    /// restricted service gateway. No provider response data crosses back.
    pub async fn probe_runtime_gateway(
        &self,
        handle: &RuntimeHandle,
        request: &crate::RuntimeGatewayProbeRequest,
    ) -> Result<crate::RuntimeGatewayProbeReceipt, AdapterError> {
        if request.version != crate::RUNTIME_GATEWAY_PROBE_VERSION
            || request.resource_id == 0
            || !valid_probe_service_id(&request.service_id)
            || !valid_probe_service_id(&request.catalog_service_id)
            || !valid_probe_user_service_slug(&request.user_service_slug)
        {
            return Err(AdapterError::Rejected(
                "runtime gateway probe request metadata is invalid".into(),
            ));
        }
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let ready = self
            .client
            .get_from_sandbox(&endpoint.endpoint, READY_PATH)
            .await?
            .unwrap_or_default();
        if !readiness_supports_runtime_gateway_probe(&ready) {
            return Err(AdapterError::Unsupported(
                "this runtime image does not advertise runtime_gateway_probe_v2".into(),
            ));
        }
        let body = serde_json::to_vec(&RuntimeGatewayProbeWireRequest {
            version: request.version,
            service_id: &request.service_id,
            catalog_service_id: &request.catalog_service_id,
            user_service_slug: &request.user_service_slug,
            resource_id: request.resource_id,
        })
        .expect("a validated gateway probe request is serializable");
        let response = self
            .client
            .post_to_sandbox_with_response(&endpoint.endpoint, RUNTIME_GATEWAY_PROBE_PATH, body)
            .await?;
        let receipt: RuntimeGatewayProbeWireReceipt =
            serde_json::from_slice(&response).map_err(|_| {
                AdapterError::Rejected(
                    "runtime returned an unreadable restricted gateway probe receipt".into(),
                )
            })?;
        if receipt.version != request.version {
            return Err(AdapterError::Rejected(
                "runtime acknowledged a different gateway probe version".into(),
            ));
        }
        Ok(crate::RuntimeGatewayProbeReceipt {
            version: receipt.version,
            outcome: receipt.outcome.into(),
        })
    }

    /// Capture the agent's conversation so it can be resumed elsewhere.
    pub async fn snapshot(&self, handle: &RuntimeHandle) -> Result<Option<Vec<u8>>, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let captured = self
            .client
            .get_from_sandbox(&endpoint.endpoint, CONVERSATION_PATH)
            .await?;
        match captured.as_ref() {
            Some(bytes) => tracing::info!(
                sandbox = %handle.0,
                bytes = bytes.len(),
                "captured this session's conversation"
            ),
            None => tracing::info!(sandbox = %handle.0, "this session has no conversation yet"),
        }
        Ok(captured)
    }

    /// Capture one exact agent conversation from a multi-conversation runtime.
    pub async fn snapshot_conversation(
        &self,
        handle: &RuntimeHandle,
        conversation_id: &str,
    ) -> Result<Option<Vec<u8>>, AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let conversation_id =
            url::form_urlencoded::byte_serialize(conversation_id.as_bytes()).collect::<String>();
        let path = format!(
            "{CONVERSATION_PATH}?{}={conversation_id}",
            crate::supervisor::CONVERSATION_QUERY_PARAMETER
        );
        self.client
            .get_from_sandbox(&endpoint.endpoint, &path)
            .await
            .map_err(runtime_operation_error)
    }

    /// Restore one captured conversation into an exact supervisor slot.
    pub async fn restore_conversation(
        &self,
        handle: &RuntimeHandle,
        conversation_slot: &str,
        snapshot: &[u8],
    ) -> Result<(), AdapterError> {
        let endpoint = self.client.endpoint(&handle.0, SUPERVISOR_PORT).await?;
        let conversation_slot =
            url::form_urlencoded::byte_serialize(conversation_slot.as_bytes()).collect::<String>();
        let path = format!(
            "{CONVERSATION_PATH}?{}={conversation_slot}",
            crate::supervisor::CONVERSATION_QUERY_PARAMETER
        );
        self.client
            .post_to_sandbox(&endpoint.endpoint, &path, snapshot.to_vec())
            .await
            .map_err(runtime_operation_error)?;
        tracing::info!(
            sandbox = %handle.0,
            slot = %conversation_slot,
            bytes = snapshot.len(),
            "restored a conversation into its runtime slot"
        );
        Ok(())
    }

    /// Every sandbox this provider is running, with what each was created for.
    pub async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        let sandboxes = self
            .client
            .list()
            .await
            .map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        Ok(sandboxes
            .into_iter()
            .filter(|sandbox| self.owns_sandbox(&sandbox.metadata))
            .map(|s| RuntimeListing {
                session: correlation_value(
                    &s.metadata,
                    SESSION_METADATA_KEY,
                    LEGACY_SESSION_METADATA_KEY,
                ),
                chat_host: s.metadata.get(CHAT_HOST_METADATA_KEY).cloned(),
                generation: runtime_generation(&s.metadata),
                created_by: correlation_value(
                    &s.metadata,
                    CREATOR_METADATA_KEY,
                    LEGACY_CREATOR_METADATA_KEY,
                ),
                // Read rather than discarded. The provider reports this and
                // this listing used to drop it, which is what let a dead
                // sandbox count as a live one.
                serving: s.is_running(),
                // Read at the same moment liveness is, from the same object.
                // Asking later would mean asking the provider again about a
                // sandbox it may by then have collected.
                lost_because: loss::loss_reason(&s),
                handle: RuntimeHandle(s.id),
            })
            .collect())
    }

    /// Tear a sandbox down.
    pub async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError> {
        // Remember the generation that is already stale if the provider says
        // this handle is absent. This snapshot is never authority to delete.
        let absent_links = self.captured_runtime_links(&handle.0).await;

        // Provider authority precedes the link snapshot used by deletion. A
        // link can take indefinitely to retire, and a provider object that
        // disappears during that wait may have its handle reused. The
        // configured reclaimer captures the exact Kubernetes
        // UID/resourceVersion; the compatibility fallback explicitly relies on
        // handles not being reused during one lifecycle operation.
        let Some(target) = self.client.capture_delete_target(&handle.0).await? else {
            for absent_link in &absent_links {
                self.retire_captured_link(&handle.0, absent_link).await;
            }
            tracing::info!(sandbox = %handle.0, "sandbox already absent");
            return Ok(());
        };
        let captured = self.captured_runtime_links(&handle.0).await;
        self.client.delete_captured(target).await?;
        for captured in &captured {
            self.retire_captured_link(&handle.0, captured).await;
        }
        tracing::info!(sandbox = %handle.0, "sandbox deleted");
        Ok(())
    }

    /// Tear down only the exact provider object carrying this identity.
    pub async fn terminate_if_identity(
        &self,
        identity: &RuntimeIdentity,
    ) -> Result<(), AdapterError> {
        let captured = self.captured_runtime_links(&identity.handle.0).await;
        self.client.delete_if_identity(identity).await?;
        for captured in &captured {
            self.retire_captured_link(&identity.handle.0, captured)
                .await;
        }
        tracing::info!(
            sandbox = %identity.handle.0,
            session_id = %identity.session,
            generation = ?identity.generation,
            "sandbox identity deleted with Kubernetes preconditions"
        );
        Ok(())
    }

    async fn retire_captured_link(&self, handle: &str, captured: &RegisteredLink) {
        let removed = {
            let mut links = self.links.lock().await;
            let key = registered_link_key(&links, handle, &captured.sender);
            key.and_then(|key| links.remove(&key))
        };
        if let Some(link) = removed {
            self.retire_removed_link(handle, link).await;
        }
    }

    async fn captured_runtime_links(&self, handle: &str) -> Vec<RegisteredLink> {
        self.links
            .lock()
            .await
            .iter()
            .filter(|(key, _)| link_key_belongs_to(key, handle))
            .map(|(_, link)| link.clone())
            .collect()
    }

    async fn retire_removed_link(&self, handle: &str, link: RegisteredLink) {
        let handle = handle.to_string();
        let switch_waiters = self.switch_waiters.clone();
        let credential_waiters = self.credential_waiters.clone();
        let runtime_policy_waiters = self.runtime_policy_waiters.clone();
        let sponsored_model_authority_waiters = self.sponsored_model_authority_waiters.clone();
        let retirement = tokio::spawn(async move {
            switch_waiters.forget(&handle, &link.sender).await;
            credential_waiters.forget_link(&handle, &link.sender).await;
            runtime_policy_waiters
                .forget_link(&handle, &link.sender)
                .await;
            sponsored_model_authority_waiters
                .forget_link(&handle, &link.sender)
                .await;
            link.retire().await;
        });
        let _ = retirement.await;
    }

    /// Find every sandbox produced for one durable reservation.
    pub async fn discover_runtime_candidates(
        &self,
        reservation: &RuntimeReservationIdentity,
    ) -> Result<Vec<RuntimeHandle>, AdapterError> {
        self.client
            .discover_runtime_candidates(reservation)
            .await
            .map_err(AdapterError::from)
    }

    /// Give a new sandbox the conversation it is taking over.
    ///
    /// A failure here fails the provisioning. The alternative is a sandbox
    /// that starts a fresh conversation while the control plane believes the
    /// old one was carried across — and the runtime it came from is about to
    /// be torn down, so nothing would notice until its owner did.
    async fn hand_over(&self, sandbox: &str, conversation: &[u8]) -> Result<(), AdapterError> {
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        self.client
            .post_to_sandbox(&endpoint.endpoint, CONVERSATION_PATH, conversation.to_vec())
            .await?;
        tracing::info!(
            %sandbox,
            bytes = conversation.len(),
            "handed this session's conversation to its new runtime"
        );
        Ok(())
    }

    /// Remember the link to a runtime, so a credential can reach it later.
    #[cfg(test)]
    pub(crate) async fn remember_link(&self, handle: &str, link: TerminalInputSender) {
        let lifecycle = self.link_lifecycle(handle);
        if lifecycle.is_current(&link).await {
            return;
        }
        // Unit-test links have no socket reader or generation supervisor. Their
        // writer is therefore the whole generation, so its retirement is the
        // truthful completion receipt replacement and termination can await.
        let retired = link.retirement();
        let (installed, _installation) = oneshot::channel();
        let _ = lifecycle.register(link, retired, installed).await;
    }

    /// The link to a runtime, if one is open.
    #[cfg(test)]
    pub(crate) async fn link_to(&self, handle: &str) -> Option<TerminalInputSender> {
        let links = self.links.lock().await;
        runtime_control_link(&links, handle).map(|registered| registered.sender.clone())
    }

    /// The registry a link's inbound task answers into.
    pub(crate) fn switch_waiters(&self) -> SwitchWaiters {
        self.switch_waiters.clone()
    }

    /// The acknowledgement registry used by the reader for credential results.
    #[cfg(test)]
    pub(crate) fn credential_waiters(&self) -> CredentialWaiters {
        self.credential_waiters.clone()
    }

    /// Registration and cleanup shared by both halves of a runtime link.
    pub(crate) fn link_lifecycle(&self, handle: &str) -> LinkLifecycle {
        self.link_lifecycle_with_key(handle, handle.to_string())
    }

    /// Registration identity for one independently supervised conversation.
    pub(crate) fn conversation_link_lifecycle(
        &self,
        handle: &str,
        conversation_slot: Option<&str>,
    ) -> LinkLifecycle {
        let key = conversation_slot.map_or_else(
            || handle.to_string(),
            |slot| format!("{handle}\0conversation\0{slot}"),
        );
        self.link_lifecycle_with_key(handle, key)
    }

    fn link_lifecycle_with_key(&self, handle: &str, registry_key: String) -> LinkLifecycle {
        LinkLifecycle {
            runtime: handle.to_string(),
            registry_key,
            links: std::sync::Arc::clone(&self.links),
            switch_waiters: self.switch_waiters.clone(),
            credential_waiters: self.credential_waiters.clone(),
            runtime_policy_waiters: self.runtime_policy_waiters.clone(),
            sponsored_model_authority_waiters: self.sponsored_model_authority_waiters.clone(),
        }
    }

    /// Wait until a sandbox reports itself running.
    async fn await_running(&self, id: &str) -> Result<Sandbox, AdapterError> {
        let deadline = std::time::Instant::now() + READY_TIMEOUT;
        loop {
            let sandbox = self.client.get(id).await?;
            if sandbox.is_running() {
                return Ok(sandbox);
            }
            if std::time::Instant::now() >= deadline {
                return Err(AdapterError::Unavailable(format!(
                    "sandbox {id} did not start within {}s",
                    READY_TIMEOUT.as_secs()
                )));
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }
}

fn readiness_supports_runtime_policy(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            let capability = value.get("githubCapability")?;
            Some(
                capability.get("name").and_then(serde_json::Value::as_str)
                    == Some("github_capability_v2")
                    && capability
                        .get("version")
                        .and_then(serde_json::Value::as_u64)
                        == Some(RUNTIME_POLICY_VERSION)
                    && capability
                        .pointer("/runtime/supported")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true),
            )
        })
        .unwrap_or(false)
}

fn require_chat_runtime_capabilities(
    body: &[u8],
    conversation_slots_required: u8,
) -> Result<(), AdapterError> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        AdapterError::Rejected("runtime returned unreadable capability readiness".into())
    })?;
    let workspace_source = value
        .pointer("/workspaceSource/version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|version| version >= WORKSPACE_SOURCE_RECEIPT_VERSION);
    if !workspace_source {
        return Err(AdapterError::Unsupported(
            "this runtime image does not advertise workspaceSource v2".into(),
        ));
    }
    let conversation_slots = value
        .pointer("/conversationSlots/max")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|maximum| maximum >= u64::from(conversation_slots_required));
    if !conversation_slots {
        return Err(AdapterError::Unsupported(format!(
            "this runtime image does not advertise conversationSlots.max >= {conversation_slots_required}"
        )));
    }
    if !readiness_supports_workspace_checkpoint(
        body,
        WorkspaceCheckpointDisposition::HoldForTeardown,
        true,
    ) {
        return Err(AdapterError::Unsupported(
            "this runtime image does not advertise cancellation-safe complete-Git workspaceCheckpoint v6"
                .into(),
        ));
    }
    if workspace_checkpoint_version(body).is_none_or(|version| version < WORKSPACE_PREEMPT_VERSION)
    {
        return Err(AdapterError::Unsupported(
            "this runtime image does not advertise foreground-preemptible workspaceCheckpoint v7"
                .into(),
        ));
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimePolicyObservationWire {
    required: bool,
    supported: bool,
    installed: bool,
    revoked: bool,
    #[serde(default)]
    launch_id: Option<String>,
    #[serde(default)]
    generation: Option<u64>,
    #[serde(default)]
    fence: Option<u64>,
    #[serde(default)]
    expires_at_epoch_seconds: Option<u64>,
    #[serde(default)]
    last_refusal: Option<String>,
}

fn parse_runtime_policy_observation(
    body: &[u8],
) -> Result<crate::RuntimePolicyObservation, AdapterError> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        AdapterError::Rejected("runtime returned unreadable policy readiness".into())
    })?;
    let capability = value.get("githubCapability").ok_or_else(|| {
        AdapterError::Unsupported(
            "this runtime image does not advertise github_capability_v2".into(),
        )
    })?;
    if capability.get("name").and_then(serde_json::Value::as_str) != Some("github_capability_v2")
        || capability
            .get("version")
            .and_then(serde_json::Value::as_u64)
            != Some(RUNTIME_POLICY_VERSION)
    {
        return Err(AdapterError::Unsupported(
            "this runtime image does not advertise the supported GitHub policy version".into(),
        ));
    }
    let runtime = capability.get("runtime").cloned().ok_or_else(|| {
        AdapterError::Rejected("runtime policy readiness omitted its runtime tuple".into())
    })?;
    let wire: RuntimePolicyObservationWire = serde_json::from_value(runtime).map_err(|_| {
        AdapterError::Rejected("runtime returned malformed policy readiness".into())
    })?;
    let last_refusal = wire
        .last_refusal
        .as_deref()
        .map(parse_runtime_policy_refusal)
        .transpose()?;
    let has_identity = wire.launch_id.is_some()
        || wire.generation.is_some()
        || wire.fence.is_some()
        || wire.expires_at_epoch_seconds.is_some();
    let valid_launch_id = wire.launch_id.as_deref().is_some_and(|value| {
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    });
    let active_tuple = wire.installed
        && !wire.revoked
        && valid_launch_id
        && wire.generation.is_some_and(|value| value > 0)
        && wire.fence.is_some_and(|value| value > 0)
        && wire.expires_at_epoch_seconds.is_some_and(|value| value > 0);
    let revoked_tuple = wire.installed
        && wire.revoked
        && valid_launch_id
        && wire.generation.is_some_and(|value| value > 0)
        && wire.fence.is_some_and(|value| value > 0)
        && wire.expires_at_epoch_seconds.is_none();
    let empty_tuple = !wire.installed && !wire.revoked && !has_identity;
    if ((wire.required || wire.installed) && !wire.supported)
        || !(active_tuple || revoked_tuple || empty_tuple)
    {
        return Err(AdapterError::Rejected(
            "runtime policy readiness tuple is inconsistent".into(),
        ));
    }
    Ok(crate::RuntimePolicyObservation {
        required: wire.required,
        supported: wire.supported,
        installed: wire.installed,
        revoked: wire.revoked,
        launch_id: wire.launch_id,
        generation: wire.generation,
        fence: wire.fence,
        expires_at_epoch_seconds: wire.expires_at_epoch_seconds,
        last_refusal,
    })
}

fn parse_runtime_policy_refusal(value: &str) -> Result<crate::RuntimePolicyRefusal, AdapterError> {
    use crate::RuntimePolicyRefusal as Refusal;
    match value {
        "missing_policy" => Ok(Refusal::MissingPolicy),
        "unknown_version" => Ok(Refusal::UnknownVersion),
        "expired" => Ok(Refusal::Expired),
        "stale_generation" => Ok(Refusal::StaleGeneration),
        "revoked" => Ok(Refusal::Revoked),
        "malformed_request" => Ok(Refusal::MalformedRequest),
        "resource_mismatch" => Ok(Refusal::ResourceMismatch),
        "operation_denied" => Ok(Refusal::OperationDenied),
        "audience_mismatch" => Ok(Refusal::AudienceMismatch),
        "signature_invalid" => Ok(Refusal::SignatureInvalid),
        _ => Err(AdapterError::Rejected(
            "runtime policy readiness used an unknown refusal code".into(),
        )),
    }
}

fn readiness_supports_runtime_gateway_probe(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            let capability = value.get("runtimeGatewayProbe")?;
            Some(
                capability.get("name").and_then(serde_json::Value::as_str)
                    == Some("runtime_gateway_probe_v2")
                    && capability
                        .get("version")
                        .and_then(serde_json::Value::as_u64)
                        == Some(RUNTIME_GATEWAY_PROBE_VERSION)
                    && capability
                        .get("supported")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true),
            )
        })
        .unwrap_or(false)
}

fn valid_probe_service_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_probe_user_service_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(serde::Serialize)]
struct RuntimeGatewayProbeWireRequest<'a> {
    version: u32,
    service_id: &'a str,
    catalog_service_id: &'a str,
    user_service_slug: &'a str,
    resource_id: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeGatewayProbeWireReceipt {
    version: u32,
    outcome: RuntimeGatewayProbeWireOutcome,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeGatewayProbeWireOutcome {
    Allowed,
    CommandSandboxInitializationFailed,
    GatewayNetworkInitializationFailed,
    PolicyDenied,
    ProviderDenied,
    TransientBrokerFailure,
}

impl From<RuntimeGatewayProbeWireOutcome> for crate::RuntimeGatewayProbeOutcome {
    fn from(value: RuntimeGatewayProbeWireOutcome) -> Self {
        match value {
            RuntimeGatewayProbeWireOutcome::Allowed => Self::Allowed,
            RuntimeGatewayProbeWireOutcome::CommandSandboxInitializationFailed => {
                Self::CommandSandboxInitializationFailed
            }
            RuntimeGatewayProbeWireOutcome::GatewayNetworkInitializationFailed => {
                Self::GatewayNetworkInitializationFailed
            }
            RuntimeGatewayProbeWireOutcome::PolicyDenied => Self::PolicyDenied,
            RuntimeGatewayProbeWireOutcome::ProviderDenied => Self::ProviderDenied,
            RuntimeGatewayProbeWireOutcome::TransientBrokerFailure => Self::TransientBrokerFailure,
        }
    }
}

fn workspace_checkpoint_version(body: &[u8]) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/workspaceCheckpoint/version")
                .and_then(serde_json::Value::as_u64)
        })
}

fn readiness_supports_workspace_checkpoint(
    body: &[u8],
    disposition: WorkspaceCheckpointDisposition,
    include_git_metadata: bool,
) -> bool {
    let minimum = match disposition {
        WorkspaceCheckpointDisposition::HoldForTeardown => WORKSPACE_HOLD_VERSION,
        WorkspaceCheckpointDisposition::ResumeAfterCapture if include_git_metadata => {
            WORKSPACE_COMPLETE_GIT_VERSION
        }
        WorkspaceCheckpointDisposition::ResumeAfterCapture => WORKSPACE_VERSION,
    };
    workspace_checkpoint_version(body).is_some_and(|version| version >= minimum)
}

fn workspace_writer_resume_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

#[derive(serde::Serialize)]
struct WorkspaceWriterResumeRequest<'a> {
    resume_token: &'a str,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceWriterResumeReceipt {
    resume_token: String,
    writers_resumed: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCheckpointPreemptReceipt {
    cancellation_requested: bool,
}

async fn resume_workspace_writers_with_client(
    client: &SandboxClient,
    endpoint: &str,
    resume_token: &str,
) -> Result<(), AdapterError> {
    let body = serde_json::to_vec(&WorkspaceWriterResumeRequest { resume_token })
        .expect("a workspace writer resume request is serializable");
    let response = client
        .post_json_to_sandbox_with_response(endpoint, WORKSPACE_CHECKPOINT_RESUME_PATH, body)
        .await?;
    let receipt: WorkspaceWriterResumeReceipt =
        serde_json::from_slice(&response).map_err(|_| {
            AdapterError::Rejected("runtime returned an invalid writer resume receipt".into())
        })?;
    if !receipt.writers_resumed || receipt.resume_token != resume_token {
        return Err(AdapterError::Rejected(
            "runtime did not acknowledge the exact writer resume token".into(),
        ));
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct WorkspaceSetupRequest<'a> {
    commands: &'a [String],
}

#[derive(Debug, serde::Deserialize)]
struct WorkspaceAcknowledgement {
    workspace_epoch: u64,
    source_revision: String,
    baseline_manifest_digest: String,
    manifest_digest: String,
    entries: usize,
    bytes: u64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointAcknowledgement {
    cma_checkpoint: u32,
    workspace_epoch: u64,
    checkpoint_version: u64,
    source_revision: String,
    manifest_digest: String,
    archive_sha256: String,
    entries: usize,
    bytes: u64,
    quiesced_writers: u32,
    #[serde(default)]
    includes_git_metadata: bool,
    writers_resumed: bool,
    #[serde(default)]
    writer_state: Option<CheckpointWriterState>,
    #[serde(default)]
    writer_resume_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CheckpointWriterState {
    Resumed,
    HeldForTeardown,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointArchiveHeader {
    cma_workspace: u32,
    workspace_epoch: u64,
    #[serde(default)]
    checkpoint_version: u64,
    #[serde(default)]
    quiesced_writers: u32,
    #[serde(default)]
    includes_git_metadata: bool,
    source_revision: String,
    manifest_digest: String,
    entries: Vec<CheckpointArchiveEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CheckpointArchiveEntryKind {
    Regular,
    Symlink,
    Submodule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CheckpointContentCompression {
    Identity,
    Zlib,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointArchiveEntry {
    path: String,
    kind: CheckpointArchiveEntryKind,
    mode: u32,
    size: u64,
    content_sha256: String,
    provider_object_id: Option<String>,
    offset: u64,
    #[serde(default)]
    stored_size: Option<u64>,
    #[serde(default)]
    compression: Option<CheckpointContentCompression>,
}

fn parse_workspace_checkpoint(
    mut wire: Vec<u8>,
    request: &WorkspaceCheckpointRequest,
) -> Result<WorkspaceCheckpointPackage, AdapterError> {
    let (acknowledgement_bytes, archive) =
        open_length_delimited(&wire, 64 * 1024, "workspace checkpoint acknowledgement")?;
    let archive_offset = wire.len().saturating_sub(archive.len());
    let acknowledgement: CheckpointAcknowledgement = serde_json::from_slice(acknowledgement_bytes)
        .map_err(|_| {
            AdapterError::Rejected("runtime checkpoint acknowledgement is malformed".into())
        })?;
    let (writer_state, writer_resume_token) =
        validate_checkpoint_writer_receipt(&acknowledgement, request.disposition)?;
    if !matches!(
        acknowledgement.cma_checkpoint,
        LEGACY_WORKSPACE_ARCHIVE_VERSION | WORKSPACE_ARCHIVE_VERSION
    ) || acknowledgement.workspace_epoch != request.epoch
        || acknowledgement.checkpoint_version != request.version
        || !acknowledgement
            .source_revision
            .eq_ignore_ascii_case(&request.source_revision)
        || !is_hex(&acknowledgement.manifest_digest, 64)
        || !is_hex(&acknowledgement.archive_sha256, 64)
        || acknowledgement.entries > MAX_WORKSPACE_FILES
        || acknowledgement.bytes > MAX_WORKSPACE_BYTES as u64
        || acknowledgement.includes_git_metadata != request.include_git_metadata
        || hex(&Sha256::digest(archive)) != acknowledgement.archive_sha256
    {
        return Err(AdapterError::Rejected(
            "runtime checkpoint acknowledgement did not prove the requested artifact".into(),
        ));
    }
    let (header_bytes, payload) = open_length_delimited(
        archive,
        MAX_WORKSPACE_HEADER_BYTES,
        "workspace checkpoint archive header",
    )?;
    let header: CheckpointArchiveHeader = serde_json::from_slice(header_bytes).map_err(|_| {
        AdapterError::Rejected("runtime checkpoint archive header is malformed".into())
    })?;
    if header.cma_workspace != acknowledgement.cma_checkpoint
        || header.workspace_epoch != request.epoch
        || header.checkpoint_version != request.version
        || header.quiesced_writers != acknowledgement.quiesced_writers
        || header.includes_git_metadata != acknowledgement.includes_git_metadata
        || !header
            .source_revision
            .eq_ignore_ascii_case(&request.source_revision)
        || header.manifest_digest != acknowledgement.manifest_digest
        || header.entries.len() != acknowledgement.entries
    {
        return Err(AdapterError::Rejected(
            "runtime checkpoint archive does not match its acknowledgement".into(),
        ));
    }

    let mut ranges = BTreeMap::<u64, (u64, usize)>::new();
    let mut legacy_expected_offset = 0_u64;
    let mut logical_bytes = 0_u64;
    for (index, entry) in header.entries.iter().enumerate() {
        if !is_hex(&entry.content_sha256, 64)
            || entry
                .provider_object_id
                .as_deref()
                .is_some_and(|object| !is_hex(object, 40))
        {
            return Err(AdapterError::Rejected(
                "runtime checkpoint archive entry metadata is invalid".into(),
            ));
        }
        logical_bytes = logical_bytes.checked_add(entry.size).ok_or_else(|| {
            AdapterError::Rejected("runtime checkpoint archive size overflowed".into())
        })?;
        if logical_bytes > MAX_WORKSPACE_BYTES as u64 {
            return Err(AdapterError::Rejected(
                "runtime checkpoint archive exceeds its byte bound".into(),
            ));
        }
        let (stored_size, compression) = checkpoint_entry_storage(header.cma_workspace, entry)?;
        let end_offset = entry.offset.checked_add(stored_size).ok_or_else(|| {
            AdapterError::Rejected("runtime checkpoint archive size overflowed".into())
        })?;
        if end_offset > MAX_WORKSPACE_BYTES as u64 {
            return Err(AdapterError::Rejected(
                "runtime checkpoint archive exceeds its byte bound".into(),
            ));
        }
        let verify_block = if header.cma_workspace == LEGACY_WORKSPACE_ARCHIVE_VERSION {
            if entry.offset != legacy_expected_offset {
                return Err(AdapterError::Rejected(
                    "runtime checkpoint archive entry metadata is invalid".into(),
                ));
            }
            legacy_expected_offset = end_offset;
            true
        } else if stored_size == 0 {
            true
        } else {
            match ranges.insert(entry.offset, (end_offset, index)) {
                Some((existing_end, existing_index)) => {
                    let existing = &header.entries[existing_index];
                    if existing_end != end_offset
                        || existing.size != entry.size
                        || existing.content_sha256 != entry.content_sha256
                        || existing.compression != entry.compression
                        || existing.stored_size != entry.stored_size
                    {
                        return Err(AdapterError::Rejected(
                            "runtime checkpoint archive content ranges conflict".into(),
                        ));
                    }
                    false
                }
                None => true,
            }
        };
        let stored = checkpoint_stored_bytes(header.cma_workspace, entry, payload)?;
        if verify_block {
            verify_checkpoint_content(stored, compression, entry.size, &entry.content_sha256)?;
        }
    }
    let physical_bytes = if header.cma_workspace == LEGACY_WORKSPACE_ARCHIVE_VERSION {
        legacy_expected_offset
    } else {
        let mut expected = 0_u64;
        for (start, (end, _)) in ranges {
            if start != expected {
                return Err(AdapterError::Rejected(
                    "runtime checkpoint archive content ranges are invalid".into(),
                ));
            }
            expected = end;
        }
        expected
    };
    if usize::try_from(physical_bytes).ok() != Some(payload.len())
        || logical_bytes != acknowledgement.bytes
    {
        return Err(AdapterError::Rejected(
            "runtime checkpoint archive payload length is invalid".into(),
        ));
    }

    let mut entries = Vec::with_capacity(header.entries.len());
    for entry in header.entries {
        let (kind, symlink_target) = match entry.kind {
            CheckpointArchiveEntryKind::Regular => (WorkspaceEntryKind::Regular, None),
            CheckpointArchiveEntryKind::Symlink => (
                WorkspaceEntryKind::Symlink,
                Some(decode_checkpoint_inline(
                    header.cma_workspace,
                    &entry,
                    payload,
                )?),
            ),
            CheckpointArchiveEntryKind::Submodule => {
                return Err(AdapterError::Rejected(
                    "runtime checkpoint contains an unsupported submodule".into(),
                ))
            }
        };
        entries.push(WorkspaceEntryPackage {
            path: entry.path,
            kind,
            mode: entry.mode,
            size: entry.size,
            content_sha256: entry.content_sha256,
            provider_object_id: entry.provider_object_id,
            symlink_target,
        });
    }
    wire.drain(..archive_offset);
    Ok(WorkspaceCheckpointPackage {
        epoch: request.epoch,
        version: request.version,
        source_revision: header.source_revision,
        manifest_digest: header.manifest_digest,
        archive_sha256: acknowledgement.archive_sha256,
        entries,
        total_bytes: acknowledgement.bytes,
        quiesced_writers: acknowledgement.quiesced_writers,
        writer_state,
        writer_resume_token,
        includes_git_metadata: acknowledgement.includes_git_metadata,
        archive: wire,
    })
}

fn checkpoint_entry_storage(
    archive_version: u32,
    entry: &CheckpointArchiveEntry,
) -> Result<(u64, CheckpointContentCompression), AdapterError> {
    match archive_version {
        LEGACY_WORKSPACE_ARCHIVE_VERSION => {
            if entry.stored_size.is_some() || entry.compression.is_some() {
                return Err(AdapterError::Rejected(
                    "legacy runtime checkpoint contains v2 content metadata".into(),
                ));
            }
            Ok((entry.size, CheckpointContentCompression::Identity))
        }
        WORKSPACE_ARCHIVE_VERSION => {
            let stored_size = entry.stored_size.ok_or_else(|| {
                AdapterError::Rejected("runtime checkpoint archive content size is missing".into())
            })?;
            let compression = entry.compression.ok_or_else(|| {
                AdapterError::Rejected("runtime checkpoint archive compression is missing".into())
            })?;
            match compression {
                CheckpointContentCompression::Identity if stored_size == entry.size => {}
                CheckpointContentCompression::Zlib if stored_size < entry.size => {}
                _ => {
                    return Err(AdapterError::Rejected(
                        "runtime checkpoint archive content encoding is non-canonical".into(),
                    ))
                }
            }
            Ok((stored_size, compression))
        }
        _ => Err(AdapterError::Rejected(
            "runtime checkpoint archive version is unsupported".into(),
        )),
    }
}

fn checkpoint_stored_bytes<'a>(
    archive_version: u32,
    entry: &CheckpointArchiveEntry,
    payload: &'a [u8],
) -> Result<&'a [u8], AdapterError> {
    let (stored_size, _) = checkpoint_entry_storage(archive_version, entry)?;
    let start = usize::try_from(entry.offset).map_err(|_| {
        AdapterError::Rejected("runtime checkpoint archive offset is unsupported".into())
    })?;
    let size = usize::try_from(stored_size).map_err(|_| {
        AdapterError::Rejected("runtime checkpoint archive size is unsupported".into())
    })?;
    let end = start.checked_add(size).ok_or_else(|| {
        AdapterError::Rejected("runtime checkpoint archive size overflowed".into())
    })?;
    payload.get(start..end).ok_or_else(|| {
        AdapterError::Rejected("runtime checkpoint archive entry is truncated".into())
    })
}

fn verify_checkpoint_content(
    stored: &[u8],
    compression: CheckpointContentCompression,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), AdapterError> {
    let actual_sha256 = match compression {
        CheckpointContentCompression::Identity => hash_checkpoint_content(stored, expected_size)?,
        CheckpointContentCompression::Zlib => {
            let mut decoder = ZlibDecoder::new(stored);
            let digest = hash_checkpoint_content(&mut decoder, expected_size)?;
            if decoder.total_in() != stored.len() as u64 {
                return Err(AdapterError::Rejected(
                    "runtime checkpoint archive content has trailing compressed bytes".into(),
                ));
            }
            digest
        }
    };
    if actual_sha256 != expected_sha256 {
        return Err(AdapterError::Rejected(
            "runtime checkpoint archive entry failed integrity verification".into(),
        ));
    }
    Ok(())
}

fn hash_checkpoint_content(
    mut reader: impl std::io::Read,
    expected_size: u64,
) -> Result<String, AdapterError> {
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| {
            AdapterError::Rejected("runtime checkpoint archive content could not be decoded".into())
        })?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            AdapterError::Rejected("runtime checkpoint archive size overflowed".into())
        })?;
        if total > expected_size {
            return Err(AdapterError::Rejected(
                "runtime checkpoint archive content exceeds its declared size".into(),
            ));
        }
        digest.update(&buffer[..read]);
    }
    if total != expected_size {
        return Err(AdapterError::Rejected(
            "runtime checkpoint archive content has the wrong decoded size".into(),
        ));
    }
    Ok(hex(&digest.finalize()))
}

fn decode_checkpoint_inline(
    archive_version: u32,
    entry: &CheckpointArchiveEntry,
    payload: &[u8],
) -> Result<String, AdapterError> {
    if entry.size > MAX_WORKSPACE_INLINE_BYTES {
        return Err(AdapterError::Rejected(
            "runtime checkpoint symlink target exceeds its bound".into(),
        ));
    }
    let (stored_size, compression) = checkpoint_entry_storage(archive_version, entry)?;
    let stored = checkpoint_stored_bytes(archive_version, entry, payload)?;
    let mut decoded = Vec::with_capacity(entry.size as usize);
    match compression {
        CheckpointContentCompression::Identity => decoded.extend_from_slice(stored),
        CheckpointContentCompression::Zlib => {
            let mut decoder = ZlibDecoder::new(stored);
            decoder.read_to_end(&mut decoded).map_err(|_| {
                AdapterError::Rejected(
                    "runtime checkpoint symlink target could not be decoded".into(),
                )
            })?;
            if decoder.total_in() != stored_size {
                return Err(AdapterError::Rejected(
                    "runtime checkpoint archive content has trailing compressed bytes".into(),
                ));
            }
        }
    }
    if decoded.len() as u64 != entry.size || hex(&Sha256::digest(&decoded)) != entry.content_sha256
    {
        return Err(AdapterError::Rejected(
            "runtime checkpoint symlink target failed integrity verification".into(),
        ));
    }
    String::from_utf8(decoded).map_err(|_| {
        AdapterError::Rejected("runtime checkpoint symlink target is not UTF-8".into())
    })
}

fn validate_checkpoint_writer_receipt(
    acknowledgement: &CheckpointAcknowledgement,
    disposition: WorkspaceCheckpointDisposition,
) -> Result<(WorkspaceWriterState, Option<String>), AdapterError> {
    match disposition {
        WorkspaceCheckpointDisposition::ResumeAfterCapture => {
            if !acknowledgement.writers_resumed
                || acknowledgement
                    .writer_state
                    .is_some_and(|state| state != CheckpointWriterState::Resumed)
                || acknowledgement.writer_resume_token.is_some()
            {
                return Err(AdapterError::Rejected(
                    "runtime did not prove that checkpoint writers resumed".into(),
                ));
            }
            Ok((WorkspaceWriterState::Resumed, None))
        }
        WorkspaceCheckpointDisposition::HoldForTeardown => {
            let token = acknowledgement.writer_resume_token.as_ref();
            if acknowledgement.writers_resumed
                || acknowledgement.writer_state != Some(CheckpointWriterState::HeldForTeardown)
                || token.is_none_or(|token| {
                    token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return Err(AdapterError::Rejected(
                    "runtime did not prove an exact non-expiring hold on checkpoint writers".into(),
                ));
            }
            Ok((WorkspaceWriterState::HeldForTeardown, token.cloned()))
        }
    }
}

fn open_length_delimited<'a>(
    bytes: &'a [u8],
    max_header: usize,
    name: &str,
) -> Result<(&'a [u8], &'a [u8]), AdapterError> {
    let prefix: [u8; 8] = bytes
        .get(..8)
        .ok_or_else(|| AdapterError::Rejected(format!("{name} has no length")))?
        .try_into()
        .map_err(|_| AdapterError::Rejected(format!("{name} length is malformed")))?;
    let length = usize::try_from(u64::from_be_bytes(prefix))
        .map_err(|_| AdapterError::Rejected(format!("{name} is too large")))?;
    if length == 0 || length > max_header {
        return Err(AdapterError::Rejected(format!("{name} exceeds its bound")));
    }
    let end = 8_usize
        .checked_add(length)
        .ok_or_else(|| AdapterError::Rejected(format!("{name} length overflowed")))?;
    let header = bytes
        .get(8..end)
        .ok_or_else(|| AdapterError::Rejected(format!("{name} is truncated")))?;
    Ok((header, &bytes[end..]))
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[path = "hosted_tests.rs"]
mod hosted_tests;

/// The socket a conversation rides, exposed for control frames alone.
///
/// A credential travels to a runtime as a control frame on a link the adapter
/// has recorded. Recording one used to happen in exactly one place — attaching
/// a terminal — so a conversation session had no recorded link, and every
/// attempt to give it a credential failed with "has no open link to carry a
/// credential". Two of the three kernels serve only conversations, so for
/// those there was no path to a credential at all.
///
/// Only credentials and plane switches are written here. Anything else
/// arriving on this sender is dropped loudly rather than framed onto a lane it
/// does not belong to: the conversation itself has its own typed channel, and
/// terminal bytes have no meaning on a socket nobody is painting.
pub(crate) fn control_lane<S>(
    sink: std::sync::Arc<tokio::sync::Mutex<S>>,
    lifecycle: Option<LinkLifecycle>,
) -> TerminalInputSender
where
    S: futures_util::SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin + Send + 'static,
{
    TerminalInputSender::spawn(CONTROL_BUFFER, move |input| {
        let sink = std::sync::Arc::clone(&sink);
        let lifecycle = lifecycle.clone();
        async move {
            let credential_operation = match &input {
                TerminalInput::Credential { operation_id, .. } => Some(operation_id.clone()),
                _ => None,
            };
            let policy_operation = match &input {
                TerminalInput::RuntimePolicy { operation_id, .. } => Some(operation_id.clone()),
                _ => None,
            };
            let sponsored_model_operation = match &input {
                TerminalInput::SponsoredModelAuthority { operation_id, .. } => {
                    Some(operation_id.clone())
                }
                _ => None,
            };
            let message = match input {
                TerminalInput::Credential {
                    operation_id,
                    credential,
                } => tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({
                        "t": "credential",
                        "operation_id": operation_id,
                        "value": credential.expose(),
                        "custody_generation": credential.custody_generation().map(std::num::NonZeroU64::get),
                    })
                    .to_string(),
                ),
                TerminalInput::RuntimePolicy {
                    operation_id,
                    envelope,
                } => tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({
                        "t": "github_capability",
                        "operation_id": operation_id,
                        "envelope": envelope,
                    })
                    .to_string(),
                ),
                TerminalInput::SponsoredModelAuthority {
                    operation_id,
                    authority,
                } => tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({
                        "t": "sponsored_model_authority",
                        "operation_id": operation_id,
                        "authority": authority,
                    })
                    .to_string(),
                ),
                TerminalInput::SwitchPlane { command } => {
                    tokio_tungstenite::tungstenite::Message::Text(
                        serde_json::json!({ "t": "switch_plane", "command": command }).to_string(),
                    )
                }
                _ => {
                    return Err(AdapterError::Rejected(
                        "a conversation control lane carries credentials and plane switches only"
                            .into(),
                    ));
                }
            };
            // Never logged with a credential's value, here or anywhere below this line.
            if sink.lock().await.send(message).await.is_err() {
                tracing::warn!("a conversation's control lane closed");
                if let (Some(operation_id), Some(lifecycle)) =
                    (credential_operation, lifecycle.as_ref())
                {
                    lifecycle.fail_credential(&operation_id).await;
                }
                if let (Some(operation_id), Some(lifecycle)) =
                    (policy_operation, lifecycle.as_ref())
                {
                    lifecycle.fail_runtime_policy(&operation_id).await;
                }
                if let (Some(operation_id), Some(lifecycle)) =
                    (sponsored_model_operation, lifecycle.as_ref())
                {
                    lifecycle
                        .fail_sponsored_model_authority(&operation_id)
                        .await;
                }
                return Err(AdapterError::Unavailable(
                    "the conversation control link write failed".into(),
                ));
            }
            Ok(())
        }
    })
}

/// How many control frames may queue for one runtime.
///
/// Small on purpose: the only thing that travels here is a credential, and a
/// backlog of them means the newest is the only one worth sending.
const CONTROL_BUFFER: usize = 4;
