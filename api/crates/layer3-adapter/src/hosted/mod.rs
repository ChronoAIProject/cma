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
mod loss;
mod telemetry;
pub(super) mod turn;

use std::collections::BTreeMap;

use tokio::sync::{mpsc, oneshot};

use crate::sandbox::{ImageSpec, Sandbox, SandboxClient, SandboxError, SandboxRequest};
use crate::{
    AdapterError, RuntimeCapacity, RuntimeHandle, RuntimeListing, RuntimeSpec, SkillPackage,
    TerminalInput,
};

pub use deployment::{Deployment, PLACEHOLDER_CREDENTIAL};

/// Port the supervisor listens on inside the sandbox.
pub(crate) const SUPERVISOR_PORT: u16 = 9090;

/// Canonical metadata key naming the control plane that created a sandbox.
///
/// Written at provisioning and read by reconciliation, so a provider shared
/// between control planes can be told apart. Absent on sandboxes created before
/// this was recorded, which reconciliation treats as its own business to decide.
const CREATOR_METADATA_KEY: &str = "cma/control-plane";

/// Historical control-plane key, kept while old-only sandboxes remain live.
const LEGACY_CREATOR_METADATA_KEY: &str = "chrono-bot/control-plane";

/// Canonical metadata key correlating a sandbox back to its session.
///
/// Written at provisioning and read by reconciliation, so the two must agree;
/// naming it once is what keeps them in step.
const SESSION_METADATA_KEY: &str = "cma/session";

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

/// Where a pod says what it is and what it can take.
const READY_PATH: &str = "/readyz";

/// How long to wait for a freshly created sandbox to start serving.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// How often to re-check a sandbox that is not yet running.
const READY_POLL: std::time::Duration = std::time::Duration::from_secs(2);

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
            image: "chrono-bot-runtime:dev".into(),
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
    link: mpsc::WeakSender<TerminalInput>,
    answer: oneshot::Sender<bool>,
}

impl SwitchWaiters {
    /// Register interest in the next answer from this link generation.
    pub(crate) async fn expect(
        &self,
        handle: &str,
        link: &mpsc::Sender<TerminalInput>,
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
    pub(crate) async fn settle(
        &self,
        handle: &str,
        link: &mpsc::Sender<TerminalInput>,
        accepted: bool,
    ) {
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
    pub(crate) async fn forget(&self, handle: &str, link: &mpsc::Sender<TerminalInput>) {
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
    links: std::sync::Arc<tokio::sync::Mutex<BTreeMap<String, mpsc::Sender<TerminalInput>>>>,
    /// Who is waiting to hear whether a plane switch was performed.
    ///
    /// A switch is only recorded once the runtime says it happened (#289):
    /// the frame reaching the socket says nothing about the program actually
    /// being swapped, and a supervisor too old to know the frame ignores it
    /// in silence. One waiter per runtime, because a session has one plane
    /// and cannot be moved twice at once.
    switch_waiters: SwitchWaiters,
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
        if !spec.skills.is_empty() || !self.agent_notes.trim().is_empty() {
            self.install_prelude(
                &sandbox.id,
                &spec.skills,
                &spec.platform_briefing,
                &spec.environment.agent_notes,
            )
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
        platform_briefing: &str,
        owner_notes: &str,
    ) -> Result<(), AdapterError> {
        let root = self.skills_root.trim();
        if !skills.is_empty() && root.is_empty() {
            return Err(AdapterError::Unsupported(
                "this runtime's agent reads no skills".to_string(),
            ));
        }
        let endpoint = self.client.endpoint(sandbox, SUPERVISOR_PORT).await?;
        if !self.takes_a_prelude(&endpoint.endpoint).await {
            return Err(AdapterError::Unsupported(format!(
                "this runtime's image cannot be given skills, so {} would be missing",
                skills
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let mut entries = Vec::new();
        let mut bodies = Vec::new();
        let notes = self.agent_notes.trim();
        if !notes.is_empty() {
            let body = self.agent_note(platform_briefing, owner_notes).into_bytes();
            entries.push(serde_json::json!({ "path": notes, "len": body.len() }));
            bodies.push(body);
        }
        for skill in skills {
            for (relative, body) in &skill.files {
                entries.push(serde_json::json!({
                    "path": format!("{root}/{}/{relative}", skill.name),
                    "len": body.len(),
                }));
                bodies.push(body.clone());
            }
        }
        let mut pack = serde_json::json!({ "chronoPrelude": 1, "files": entries })
            .to_string()
            .into_bytes();
        pack.push(b'\n');
        for body in bodies {
            pack.extend_from_slice(body.as_slice());
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
    async fn takes_a_prelude(&self, endpoint: &str) -> bool {
        let Ok(body) = self.client.get_from_sandbox(endpoint, READY_PATH).await else {
            return false;
        };
        serde_json::from_slice::<serde_json::Value>(body.unwrap_or_default().as_slice())
            .ok()
            .and_then(|v| v.get("prelude").and_then(serde_json::Value::as_bool))
            .unwrap_or(false)
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
        // Dropped first: the link belongs to a sandbox that is about to stop
        // existing, and keeping it would leave this map growing with the
        // remains of every session this process has ever served.
        let link = self.links.lock().await.remove(&handle.0);
        if let Some(link) = link {
            self.switch_waiters.forget(&handle.0, &link).await;
        }
        self.client.delete(&handle.0).await?;
        tracing::info!(sandbox = %handle.0, "sandbox deleted");
        Ok(())
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
    pub(crate) async fn remember_link(&self, handle: &str, link: mpsc::Sender<TerminalInput>) {
        let previous = {
            let mut links = self.links.lock().await;
            if links
                .get(handle)
                .is_some_and(|current| current.same_channel(&link))
            {
                return;
            }
            links.insert(handle.to_string(), link)
        };
        if let Some(previous) = previous {
            // Replacing a generation invalidates any request still waiting on
            // that old socket. Scope the cancellation so a successor that has
            // already registered its own waiter remains intact.
            self.switch_waiters.forget(handle, &previous).await;
        }
    }

    /// The link to a runtime, if one is open.
    pub(crate) async fn link_to(&self, handle: &str) -> Option<mpsc::Sender<TerminalInput>> {
        self.links.lock().await.get(handle).cloned()
    }

    /// The registry a link's inbound task answers into.
    pub(crate) fn switch_waiters(&self) -> SwitchWaiters {
        self.switch_waiters.clone()
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
/// Only a credential is written here. Anything else arriving on this sender is
/// dropped loudly rather than framed onto a lane it does not belong to: the
/// conversation itself has its own typed channel, and terminal bytes have no
/// meaning on a socket nobody is painting.
pub(crate) fn control_lane<S>(
    sink: std::sync::Arc<tokio::sync::Mutex<S>>,
) -> mpsc::Sender<TerminalInput>
where
    S: futures_util::SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<TerminalInput>(CONTROL_BUFFER);
    tokio::spawn(async move {
        while let Some(input) = rx.recv().await {
            let message = match input {
                TerminalInput::Credential(value) => tokio_tungstenite::tungstenite::Message::Text(
                    serde_json::json!({ "t": "credential", "value": value }).to_string(),
                ),
                TerminalInput::SwitchPlane { command } => {
                    tokio_tungstenite::tungstenite::Message::Text(
                        serde_json::json!({ "t": "switch_plane", "command": command }).to_string(),
                    )
                }
                _ => {
                    tracing::warn!(
                        "a conversation's control lane carries credentials and plane switches only"
                    );
                    continue;
                }
            };
            // Never logged with a credential's value, here or anywhere below this line.
            if sink.lock().await.send(message).await.is_err() {
                tracing::warn!("a conversation's control lane closed");
                break;
            }
        }
    });
    tx
}

/// How many control frames may queue for one runtime.
///
/// Small on purpose: the only thing that travels here is a credential, and a
/// backlog of them means the newest is the only one worth sending.
const CONTROL_BUFFER: usize = 4;
