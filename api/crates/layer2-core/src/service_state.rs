//! State owned by the core session service.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use layer3_adapter::{KernelAdapter, PermissionLocator, RuntimeHandle, TerminalInputSender};
use tokio::sync::{broadcast, watch, Mutex, RwLock};

use crate::environment_service::EnvironmentProfileService;
use crate::ids::{KernelId, SessionId};
use crate::platform_service::PlatformSettingsService;
use crate::session::Session;
use crate::store::{
    ChatCheckpointStore, ContextStore, CredentialSource, HandoverStore, SessionStore,
    SkillPackStore,
};
use crate::terminal::{ScreenModel, TerminalGeometry};

/// How many output chunks a slow attached client may fall behind before it is
/// dropped from the live fan-out. It can always recover from a fresh snapshot,
/// so lagging is survivable; unbounded buffering is not.
pub(crate) const LIVE_BUFFER: usize = 512;

/// How many readings a slow subscriber may fall behind before it is dropped.
///
/// Small because each reading supersedes its predecessor whole, and kept
/// separate so their cadence can never spend the live frame lane's budget.
pub(crate) const RESOURCE_BUFFER: usize = 8;

/// Builds a screen model at a given geometry.
///
/// Injected so the core never depends on a particular terminal parser.
pub type ScreenFactory = Arc<dyn Fn(TerminalGeometry) -> Box<dyn ScreenModel> + Send + Sync>;

/// Who currently holds a session's terminal.
pub(crate) struct Attachment {
    /// Which attachment this is, among those this session has issued.
    pub(crate) generation: u64,
    /// Signals the holder that the attachment has been taken from them, and why.
    pub(crate) supersede: broadcast::Sender<crate::lease::Detached>,
}

/// Everything about a session that exists only while its runtime does.
///
/// Deliberately not persisted: a screen model, a socket and an attachment lease
/// describe a live process, and a restarted one has none of them. They are
/// rebuilt when a client attaches.
pub(crate) struct SessionEntry {
    pub(crate) session: Session,
    /// Internal Trigger provenance retained across live-entry flushes.
    pub(crate) trigger_origin: Option<crate::session::TriggerSessionOrigin>,
    /// Shared-host provenance retained across live-entry status commits.
    pub(crate) chat_origin: Option<crate::store::ChatOrigin>,
    /// Durable Trigger prompt outbox state mirrored into the live entry.
    pub(crate) initial_prompt: Option<crate::store::InitialPromptDispatch>,
    /// This session's durable FIFO, kept alive while its runtime is live.
    pub(crate) event_log: Arc<crate::log::SessionLog>,
    /// Owner-delegated credential last handed to the runtime. Held so a refresh
    /// can be pushed without the caller tracking which sessions exist.
    pub(crate) subject_credential: Option<String>,
    /// The values the session was handed as environment, kept so a tool body
    /// can be redacted of them before it is written. Values only: the core
    /// needs to replace an exact value wherever a tool echoes one, and the key
    /// is not what leaks.
    pub(crate) environment_values: Vec<String>,
    pub(crate) screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    pub(crate) runtime: Option<RuntimeHandle>,
    /// Durable authority generation to which the live runtime channels belong.
    pub(crate) runtime_generation: u64,
    /// Never-reused process-local identity of the attempt that installed this
    /// entry. A cancelled attempt can remove only its own incomplete entry even
    /// when a compatibility store has no durable runtime generation.
    pub(crate) publication_token: String,
    /// Whether every process-local consumer for this entry has been installed.
    ///
    /// An entry becomes visible before buffered conversation updates can be
    /// folded, but a cancelled or failed publication must never prove that a
    /// durable rotation is complete. Reconciliation may discard an incomplete
    /// entry; only a complete one may settle its durable rotation claim.
    pub(crate) publication_complete: bool,
    /// Process-local identity of the exact conversation link installed here.
    ///
    /// Durable generation zero is shared by every compatibility runtime, and a
    /// readoption keeps the same durable runtime generation. Neither therefore
    /// identifies one folder. This token does: an older folder can never match a
    /// replacement entry, even when both point at the same durable generation.
    pub(crate) conversation_link: Option<crate::conversation_opening::ConversationLinkId>,
    /// Store-issued identity of the one link allowed to commit inbound work.
    ///
    /// Unlike `conversation_link`, this crosses replicas. Publication advances
    /// its epoch atomically, and every conversation-originated store commit is
    /// conditioned on the exact value installed here.
    pub(crate) conversation_authority: Option<crate::coordination::ConversationLink>,
    pub(crate) input: Option<TerminalInputSender>,
    /// Where this session's stored context lives, once any has been captured.
    pub(crate) context_ref: Option<String>,
    /// Durable location of this session's exact resolved skill bytes.
    pub(crate) skill_packs: crate::store::SkillPackState,
    /// Transcript length at the last capture, so an unchanged one is not rewritten.
    pub(crate) captured_len: usize,
    /// Current runtime output. `None` after that channel ends; receivers on the
    /// previous sender then close after draining every queued byte.
    pub(crate) live: Option<broadcast::Sender<Vec<u8>>>,
    /// Process ending recorded before `live` is closed.
    pub(crate) terminal_ending: watch::Sender<Option<crate::lease::Detached>>,
    /// Liveness on the conversation plane: pieces of a message being assembled.
    ///
    /// Separate from the terminal's byte stream because the two are different
    /// kinds of thing and a session has one plane or the other. Kept typed
    /// rather than as bytes, because a token-level piece of a message is not a
    /// screen and rendering it into one would be a decision the core has no
    /// business making.
    pub(crate) live_frames: broadcast::Sender<crate::event::LiveFrame>,
    /// Resource readings as they arrive.
    ///
    /// Kept off `live_frames` so neither kind of subscriber spends the other
    /// kind's lag budget.
    pub(crate) resources: broadcast::Sender<crate::resources::ResourceReading>,
    /// The newest resource reading this session produced, and when it arrived.
    ///
    /// Overwritten on every cadence and never written to the durable event log.
    pub(crate) last_reading: Option<crate::resources::HeldReading>,
    /// Whether this session's processor use has been near its limit, and for how long.
    ///
    /// Ephemeral like the readings it folds. Only the two edges it emits are
    /// recorded, and a fresh live entry starts with a fresh fold.
    pub(crate) cpu_pressure: crate::pressure::CpuPressure,
    /// Whether this session's memory has been near its limit, and for how long.
    ///
    /// Ephemeral like the readings it folds. Only the two edges it emits are
    /// recorded, and a fresh live entry starts with a fresh fold.
    pub(crate) memory_pressure: crate::pressure::MemoryPressure,
    /// Whether this session reported that it cannot produce resource readings.
    pub(crate) readings_unreported: bool,
    /// Input toward the session's conversation, when it holds one.
    pub(crate) conversation: Option<layer3_adapter::ConversationInputSender>,
    /// The message the agent is still assembling, which one it is, and whether
    /// its adapter positively correlated it with a prompt on this link.
    ///
    /// A runtime streams a message in pieces and says nothing to mark the end
    /// of one; what marks it is the next message starting, or the turn ending.
    /// Held here so the finished message can be committed at that moment
    /// rather than each piece being written down as though it were history.
    pub(crate) assembling: Option<(Option<String>, String, bool)>,
    /// The last context reading this runtime gave, if it has given one.
    ///
    /// Held per entry rather than per session on purpose: an entry is built
    /// afresh whenever a runtime is replaced, so readings compared here come
    /// from one runtime. A replacement counting afresh cannot be mistaken for
    /// a conversation that got smaller.
    ///
    /// A control-plane restart also starts with no reading. A compaction that
    /// spans that restart is therefore not recorded; closing that narrow gap
    /// would require reading every session's complete event history on boot.
    pub(crate) context_reading: Option<u64>,
    /// Decisions the runtime is blocked on, by the identifier that answers one.
    ///
    /// Held so an answer to something nothing is waiting for is refused rather
    /// than forwarded: a runtime handed one will either ignore it or act on
    /// the wrong thing, and neither is visible to whoever answered.
    pub(crate) pending_approvals: HashMap<String, Option<PermissionLocator>>,
    /// Locator fingerprints observed in the active turn, including requests
    /// already resolved. Retained until the turn or runtime changes so a late
    /// duplicate cannot reopen an approval that was already answered.
    pub(crate) approval_locators: HashMap<String, Option<PermissionLocator>>,
    /// The live attachment, when one is held.
    pub(crate) attached: Option<Attachment>,
    /// How many attachments this session has issued.
    ///
    /// Monotonic rather than a count of live ones, so a replacement attachment
    /// is never numbered the same as the one it replaced - which is the whole
    /// basis on which a superseded holder's release is told apart from a real
    /// one.
    pub(crate) attachments_issued: u64,
}

/// What distinguishes one deployment of this service from another.
///
/// Grouped rather than passed alongside the ports because they are the same
/// kind of thing - facts about this installation - and because a constructor
/// taking eight positional arguments, most of them `Option<String>`, is one
/// where two of them eventually get swapped without the compiler noticing.
#[derive(Debug, Clone, Default)]
pub struct Deployment {
    /// The runtime version sessions are expected to be running.
    ///
    /// `None` disables rotation, which is what a deployment that does not
    /// version its runtime image wants.
    pub runtime_version: Option<String>,
    /// Who this control plane says it is, on the runtimes it creates.
    pub identity: Option<String>,
    /// Whether runtimes carrying no identity are this control plane's to judge.
    ///
    /// True for the deployment that owns a provider. False for any additional
    /// control plane sharing it, which must confine itself to what it created.
    pub adopts_untagged: bool,
    /// Trigger capabilities declared by the exact pinned runtime image.
    ///
    /// Empty is fail-closed: ordinary sessions remain available, but no Trigger
    /// Launch may claim a capability that the deployment did not explicitly pin.
    pub trigger_runtime_capabilities: Vec<crate::RequiredCapability>,
    /// Opaque kernel selected by the deployment for the closed Trigger profile.
    ///
    /// `None` disables Trigger session admission. The core never assigns a
    /// product runtime itself; the composition root must bind this identifier
    /// to an installed adapter.
    pub trigger_kernel: Option<KernelId>,
}

/// Per-session operation gates.
///
/// A gate may be held across store I/O, but the service-wide `entries` lock
/// must never be. Callers acquire a gate before `entries`; no path acquires
/// them in the opposite order. Separate registries use this shape for separate
/// ordering domains. Weak values keep one-off operations from turning either
/// registry into a second session store.
#[derive(Default)]
pub(crate) struct SessionGates {
    held: StdMutex<HashMap<SessionId, Weak<Mutex<()>>>>,
}

impl SessionGates {
    pub(crate) fn gate(&self, id: &SessionId) -> Arc<Mutex<()>> {
        let mut held = self.held.lock().unwrap_or_else(|error| error.into_inner());
        held.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = held.get(id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        held.insert(id.clone(), Arc::downgrade(&gate));
        gate
    }

    #[cfg(test)]
    pub(crate) fn registry_len(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

/// Per-session boundary between accepted input and a rotation transition.
///
/// Input holds a read lease from its final state check through delivery. A
/// rotation holds the write lease while publishing `Rotating` durably and in
/// the live entry, so an operation accepted against the old runtime finishes
/// before the handover window opens and no later operation can cross into it.
#[derive(Default)]
pub(crate) struct SessionInputGates {
    held: StdMutex<HashMap<SessionId, Weak<RwLock<()>>>>,
}

impl SessionInputGates {
    pub(crate) fn gate(&self, id: &SessionId) -> Arc<RwLock<()>> {
        let mut held = self.held.lock().unwrap_or_else(|error| error.into_inner());
        held.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = held.get(id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(RwLock::new(()));
        held.insert(id.clone(), Arc::downgrade(&gate));
        gate
    }
}

/// The core service.
pub struct SessionService {
    /// Live sessions, keyed by id. Only those with a running runtime appear.
    pub(crate) entries: RwLock<HashMap<SessionId, SessionEntry>>,
    /// Serializes lifecycle decisions and installation for one session only.
    pub(crate) lifecycle_gates: SessionGates,
    /// Fences runtime input delivery against the transition into rotation.
    pub(crate) input_gates: SessionInputGates,
    /// Serializes rebuilding one runtime attachment after process replacement.
    ///
    /// Separate from lifecycle ordering because opening a conversation may wait
    /// on a human-paced sign-in. Within this process, the permit spans the
    /// history read, network open, folder creation, installation, and durable
    /// lease release so two callers cannot leave two sockets draining and
    /// folding the same runtime conversation.
    ///
    /// Process-local exclusion is sufficient only while
    /// `infra/api/deployment.yaml` keeps `replicas: 1`. Horizontal scaling
    /// requires durable cross-process ownership before that value can grow.
    pub(crate) readoption_gates: SessionGates,
    /// Orders accepted prompts before the runtime updates they cause.
    ///
    /// Separate from lifecycle installation because a conversation starts
    /// reporting sign-in and command updates before its live entry exists.
    pub(crate) conversation_order_gates: SessionGates,
    /// Serializes prompt admission until an accepted turn is visible.
    pub(crate) prompt_gates: SessionGates,
    /// The single writer of every session's event sequence.
    ///
    /// Owned by the durable lanes, so allocation and FIFO insertion share the
    /// same per-session critical section.
    pub(crate) sequences: Arc<crate::sequence::Sequences>,
    /// Pending history and the durable-write lane for each session.
    pub(crate) event_logs: Arc<crate::log::EventLogs>,
    pub(crate) adapters: HashMap<KernelId, Arc<dyn KernelAdapter>>,
    pub(crate) screen_factory: ScreenFactory,
    /// The durable record. Source of truth for anything that outlives a process.
    pub(crate) store: Arc<dyn SessionStore>,
    /// Where an agent's accumulated context is kept, when a deployment has one.
    pub(crate) context: Option<Arc<dyn ContextStore>>,
    /// Where claim-bound compatibility handover snapshots are kept.
    pub(crate) handovers: Option<Arc<dyn HandoverStore>>,
    /// Where complete chat-host recovery objects are retained by digest.
    pub(crate) chat_checkpoints: Option<Arc<dyn ChatCheckpointStore>>,
    /// Where to get an owner's credential when the owner is not here to give one.
    pub(crate) credentials: Option<Arc<dyn CredentialSource>>,
    /// Where this session's skills are fetched from, when anything can.
    pub(crate) skills: Option<Arc<dyn crate::store::SkillSource>>,
    /// Repository bytes for Trigger sessions, reached only by the control plane.
    pub(crate) workspaces: Option<Arc<dyn crate::store::SessionWorkspaceSource>>,
    /// Where the exact skill packs fetched for a session are kept.
    pub(crate) skill_packs: Option<Arc<dyn SkillPackStore>>,
    /// Sessions whose runtime has said it cannot be carried onto a newer one.
    ///
    /// Held in memory rather than recorded, deliberately: it is a fact about a
    /// particular sandbox, not about the session, so it should not outlive the
    /// sandbox. A restarted process asks again, which is right - by then the
    /// runtime may well have been replaced by other means.
    pub(crate) rotation_refused: RwLock<HashSet<SessionId>>,
    /// Sessions whose outgoing runtime is being replaced.
    ///
    /// Unlike the durable `Rotating` status, this spans the final installation
    /// boundary where readiness is committed just before the replacement entry
    /// becomes live. It is consulted only after a live lookup misses.
    pub(crate) rotations_in_progress: RwLock<HashSet<SessionId>>,
    /// Who this control plane says it is, on the runtimes it creates.
    ///
    /// A sandbox provider is shared: one key sees everything provisioned
    /// against it, including runtimes belonging to another control plane
    /// entirely. Without an identity on each one, a second control plane reads
    /// every session it has never heard of as an orphan and tears it down.
    pub(crate) identity: Option<String>,
    /// Unique identity of this process instance for durable operation leases.
    ///
    /// Deployment identity deliberately is not enough: two replicas from one
    /// rollout share it and must still be distinguishable across lease expiry.
    pub(crate) coordination_holder: String,
    /// Whether runtimes carrying no identity are this control plane's to judge.
    ///
    /// True for the deployment that owns a provider, so runtimes created before
    /// identities were recorded are still reclaimed when they are genuinely
    /// orphaned. False for any additional control plane sharing the same
    /// provider, which must confine itself to what it created.
    pub(crate) adopts_untagged: bool,
    /// The environment profiles this service resolves sessions against.
    pub(crate) environments: Arc<EnvironmentProfileService>,
    /// Deployment settings, always present so readers cannot skip the fallback.
    pub(crate) settings: Arc<PlatformSettingsService>,
    /// Durable ownership of shared chat runtimes, absent on deployments that
    /// have not composed the chat surface.
    pub(crate) chat_hosts: Option<Arc<dyn crate::store::ChatHostStore>>,
    /// Serializes first-host creation and lifecycle claims for one owner in
    /// the deployment's currently single control-plane replica.
    pub(crate) chat_host_guards: crate::limits::OwnerGuards,
    /// Serializes one owner's limit check through its record write.
    pub(crate) session_limit_guards: crate::limits::OwnerGuards,
    /// Serializes the deployment-wide reservation check through its record write.
    ///
    /// This closes the race inside one process. Like the owner-limit guard, it is
    /// not a cross-process transaction; a scaled-out deployment needs a
    /// conditional store write to make that stronger guarantee.
    pub(crate) deployment_capacity_guard: Arc<Mutex<()>>,
    /// The runtime version sessions are expected to be running.
    ///
    /// Held by the control plane rather than asked of each sandbox: a session
    /// on anything else is carried forward. `None` disables rotation entirely,
    /// which is what a deployment that does not version its runtime image wants.
    pub(crate) runtime_version: Option<String>,
    pub(crate) trigger_runtime_capabilities: Vec<crate::RequiredCapability>,
    /// Opaque deployment-selected kernel for Trigger-owned sessions.
    pub(crate) trigger_kernel: Option<KernelId>,
    /// Sessions this process is currently bringing up.
    ///
    /// Detached bring-up can outlive its request while no live entry exists;
    /// this distinguishes it from work a reconciliation sweep must recover.
    pub(crate) starting: RwLock<HashSet<SessionId>>,
    /// Chat hosts this process is currently provisioning or resuming.
    pub(crate) chat_hosts_starting: RwLock<HashSet<crate::ChatHostId>>,
    /// Milliseconds to wait before the first provisioning retry.
    ///
    /// Held here rather than as a constant so a test can exercise the retry
    /// path without waiting out a delay meant for a provider under load.
    pub(crate) retry_backoff_ms: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    pub(crate) pause_next_live_install: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub(crate) live_install_paused: tokio::sync::Notify,
    #[cfg(test)]
    pub(crate) live_install_resume: tokio::sync::Notify,
}

/// What a session service reaches outside itself, beyond its own store.
///
/// Grouped rather than passed one by one: each is optional, each is a place
/// this crate deliberately does not know the shape of, and a positional list
/// of several `Option<Arc<...>>` values is one where two can be swapped without
/// the compiler noticing.
#[derive(Default)]
pub struct SessionPorts {
    /// Where a session's conversation is stored between runtimes.
    pub context: Option<Arc<dyn ContextStore>>,
    /// Where compatibility handover snapshots are durably staged.
    pub handovers: Option<Arc<dyn HandoverStore>>,
    /// Where chat workspace and conversation checkpoint generations are staged.
    pub chat_checkpoints: Option<Arc<dyn ChatCheckpointStore>>,
    /// Where a session's own credential is minted.
    pub credentials: Option<Arc<dyn CredentialSource>>,
    /// Where a session's skills are fetched from.
    pub skills: Option<Arc<dyn crate::store::SkillSource>>,
    /// Exact Trigger workspace source used for every runtime generation.
    pub workspaces: Option<Arc<dyn crate::store::SessionWorkspaceSource>>,
    /// Where a session's already-resolved skill packs are kept.
    pub skill_packs: Option<Arc<dyn SkillPackStore>>,
    /// The deployment's own settings, when this process has a store for them.
    ///
    /// `None` becomes `unconfigured()`, so sessions still use shipped bounds.
    pub settings: Option<Arc<PlatformSettingsService>>,
    /// Durable shared chat-host aggregates.
    pub chat_hosts: Option<Arc<dyn crate::store::ChatHostStore>>,
}
