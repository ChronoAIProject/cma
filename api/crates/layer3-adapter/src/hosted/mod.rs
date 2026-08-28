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

pub(crate) mod acp;
mod deployment;
mod handshake;
mod inbound;
mod link;
pub(crate) use link::credential_answer;
mod loss;
mod telemetry;
pub(super) mod turn;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use tokio::sync::oneshot;

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::sandbox_reclaimer::{
    CREATOR_METADATA_KEY, GENERATION_METADATA_KEY, RESERVATION_METADATA_KEY, SESSION_METADATA_KEY,
};
use crate::task_retirement::TaskRetirement;
use crate::terminal_input::WeakTerminalInputSender;
use crate::{
    AdapterError, RuntimeCapacity, RuntimeGeneration, RuntimeHandle, RuntimeIdentity,
    RuntimeListing, RuntimeReservationIdentity, RuntimeSpec, SkillPackage, TerminalInput,
    TerminalInputSender,
};

pub use deployment::{Deployment, PLACEHOLDER_CREDENTIAL};

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

/// Canonical metadata key naming which kernel a sandbox is running.
const KERNEL_METADATA_KEY: &str = "cma/kernel";

/// Historical kernel key, kept for old control planes during the transition.
const LEGACY_KERNEL_METADATA_KEY: &str = "chrono-bot/kernel";

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

/// Where a pod says what it is and what it can take.
const READY_PATH: &str = "/readyz";

/// How long to wait for a freshly created sandbox to start serving.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How often to re-check a sandbox that is not yet running.
const READY_POLL: std::time::Duration = std::time::Duration::from_secs(2);

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
            workspace: "/home/chrono".into(),
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

/// Registration and cleanup for one runtime link generation.
///
/// A generation supervisor owns reader, writer, and registry cleanup, while
/// callers use the shared map to deliver credentials and switches. Keeping the
/// completion receipt beside the sender makes replacement wait for that whole
/// supervisor and makes every close remove only its exact socket generation.
#[derive(Clone)]
pub(crate) struct LinkLifecycle {
    runtime: String,
    links: std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, RegisteredLink>>>,
    switch_waiters: SwitchWaiters,
    credential_waiters: CredentialWaiters,
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
                .get(&self.runtime)
                .is_some_and(|current| current.sender.same_channel(&link))
            {
                if link.is_closed() {
                    links.remove(&self.runtime);
                    let _ = installed.send(());
                    return false;
                }
                let _ = installed.send(());
                return true;
            }
            let previous = links.insert(self.runtime.clone(), registration);
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
            let retirement = tokio::spawn(async move {
                switch_waiters.forget(&runtime, &previous.sender).await;
                credential_waiters
                    .forget_link(&runtime, &previous.sender)
                    .await;
                previous.retire().await;
            });
            let _ = retirement.await;
        }

        let mut links = self.links.lock().await;
        let is_current = links
            .get(&self.runtime)
            .is_some_and(|current| current.sender.same_channel(&link));
        if is_current && link.is_closed() {
            links.remove(&self.runtime);
            return false;
        }
        is_current
    }

    /// Forget this generation without disturbing a successor already present.
    pub(crate) async fn forget(&self, link: &TerminalInputSender) {
        let mut links = self.links.lock().await;
        if links
            .get(&self.runtime)
            .is_some_and(|current| current.sender.same_channel(link))
        {
            links.remove(&self.runtime);
        }
        drop(links);
        // A successor may be in the map while this generation still owns the
        // previous waiter, so waiter cleanup independently checks identity.
        self.switch_waiters.forget(&self.runtime, link).await;
        self.credential_waiters
            .forget_link(&self.runtime, link)
            .await;
    }

    pub(crate) async fn forget_if_whole_retired(&self, link: &TerminalInputSender) {
        let removed = {
            let mut links = self.links.lock().await;
            let can_forget = links.get(&self.runtime).is_some_and(|current| {
                current.sender.same_channel(link) && current.retired.is_retired()
            });
            can_forget.then(|| links.remove(&self.runtime)).flatten()
        };
        if removed.is_some() {
            self.switch_waiters.forget(&self.runtime, link).await;
            self.credential_waiters
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
        self.links
            .lock()
            .await
            .get(&self.runtime)
            .is_some_and(|current| current.sender.same_channel(link) && !link.is_closed())
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
}

/// A kernel running in a provider-managed sandbox.
pub struct HostedKernel {
    pub(crate) client: SandboxClient,
    shape: SandboxShape,
    /// Which kernel this is, recorded on the sandboxes it creates.
    kernel: &'static str,
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
        Self {
            client,
            shape,
            kernel,
            skills_root: skills_root.into(),
            agent_notes: agent_notes.into(),
            links: std::sync::Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            switch_waiters: SwitchWaiters::default(),
            credential_waiters: CredentialWaiters::default(),
        }
    }

    /// Where the agent works, inside its sandbox.
    pub fn workspace(&self) -> &str {
        &self.shape.workspace
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
            timeout: self.shape.timeout_seconds,
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
        }
        Ok(readiness.prelude.unwrap_or(false))
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

    /// Every sandbox this provider is running, with what each was created for.
    pub async fn list_runtimes(&self) -> Result<Vec<RuntimeListing>, AdapterError> {
        let sandboxes = self
            .client
            .list()
            .await
            .map_err(|e| AdapterError::Unavailable(e.to_string()))?;
        Ok(sandboxes
            .into_iter()
            .map(|s| RuntimeListing {
                session: correlation_value(
                    &s.metadata,
                    SESSION_METADATA_KEY,
                    LEGACY_SESSION_METADATA_KEY,
                ),
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
        let absent_link = self.links.lock().await.get(&handle.0).cloned();

        // Provider authority precedes the link snapshot used by deletion. A
        // link can take indefinitely to retire, and a provider object that
        // disappears during that wait may have its handle reused. The
        // configured reclaimer captures the exact Kubernetes
        // UID/resourceVersion; the compatibility fallback explicitly relies on
        // handles not being reused during one lifecycle operation.
        let Some(target) = self.client.capture_delete_target(&handle.0).await? else {
            if let Some(absent_link) = absent_link.as_ref() {
                self.retire_captured_link(&handle.0, absent_link).await;
            }
            tracing::info!(sandbox = %handle.0, "sandbox already absent");
            return Ok(());
        };
        let captured = self.links.lock().await.get(&handle.0).cloned();
        self.client.delete_captured(target).await?;
        if let Some(captured) = captured.as_ref() {
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
        let captured = self.links.lock().await.get(&identity.handle.0).cloned();
        self.client.delete_if_identity(identity).await?;
        if let Some(captured) = captured.as_ref() {
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
            let is_same = links
                .get(handle)
                .is_some_and(|current| current.sender.same_channel(&captured.sender));
            is_same.then(|| links.remove(handle)).flatten()
        };
        if let Some(link) = removed {
            self.retire_removed_link(handle, link).await;
        }
    }

    async fn retire_removed_link(&self, handle: &str, link: RegisteredLink) {
        let handle = handle.to_string();
        let switch_waiters = self.switch_waiters.clone();
        let credential_waiters = self.credential_waiters.clone();
        let retirement = tokio::spawn(async move {
            switch_waiters.forget(&handle, &link.sender).await;
            credential_waiters.forget_link(&handle, &link.sender).await;
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
        self.links
            .lock()
            .await
            .get(handle)
            .map(|registered| registered.sender.clone())
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
        LinkLifecycle {
            runtime: handle.to_string(),
            links: std::sync::Arc::clone(&self.links),
            switch_waiters: self.switch_waiters.clone(),
            credential_waiters: self.credential_waiters.clone(),
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
            let message = match input {
                TerminalInput::Credential {
                    operation_id,
                    value,
                } => tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({
                        "t": "credential",
                        "operation_id": operation_id,
                        "value": value,
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
