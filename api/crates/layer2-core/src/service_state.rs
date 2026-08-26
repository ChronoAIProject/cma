//! State owned by the core session service.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use layer3_adapter::{KernelAdapter, RuntimeHandle, TerminalInput};
use tokio::sync::{broadcast, mpsc, watch, Mutex, RwLock};

use crate::environment_service::EnvironmentProfileService;
use crate::ids::{KernelId, SessionId};
use crate::platform_service::PlatformSettingsService;
use crate::session::Session;
use crate::store::{ContextStore, CredentialSource, SessionStore};
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
    pub(crate) input: Option<mpsc::Sender<TerminalInput>>,
    /// Where this session's stored context lives, once any has been captured.
    pub(crate) context_ref: Option<String>,
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
    /// Whether this session's memory has been near its limit, and for how long.
    ///
    /// Ephemeral like the readings it folds. Only the two edges it emits are
    /// recorded, and a fresh live entry starts with a fresh fold.
    pub(crate) memory_pressure: crate::pressure::MemoryPressure,
    /// Whether this session reported that it cannot produce resource readings.
    pub(crate) readings_unreported: bool,
    /// Input toward the session's conversation, when it holds one.
    pub(crate) conversation: Option<layer3_adapter::ConversationInputSender>,
    /// The message the agent is still assembling, and which one it is.
    ///
    /// A runtime streams a message in pieces and says nothing to mark the end
    /// of one; what marks it is the next message starting, or the turn ending.
    /// Held here so the finished message can be committed at that moment
    /// rather than each piece being written down as though it were history.
    pub(crate) assembling: Option<(Option<String>, String)>,
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
    pub(crate) pending_approvals: HashSet<String>,
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
}

/// Per-session desired-state and runtime-installation gates.
///
/// A gate may be held across store I/O, but the service-wide `entries` lock
/// must never be. Callers acquire a gate before `entries`; no path acquires
/// them in the opposite order. Weak values keep one-off lifecycle requests
/// from turning the registry into a second session store.
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

/// The core service.
pub struct SessionService {
    /// Live sessions, keyed by id. Only those with a running runtime appear.
    pub(crate) entries: RwLock<HashMap<SessionId, SessionEntry>>,
    /// Serializes lifecycle decisions and installation for one session only.
    pub(crate) lifecycle_gates: SessionGates,
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
    /// Where to get an owner's credential when the owner is not here to give one.
    pub(crate) credentials: Option<Arc<dyn CredentialSource>>,
    /// Where this session's skills are fetched from, when anything can.
    pub(crate) skills: Option<Arc<dyn crate::store::SkillSource>>,
    /// Sessions whose runtime has said it cannot be carried onto a newer one.
    ///
    /// Held in memory rather than recorded, deliberately: it is a fact about a
    /// particular sandbox, not about the session, so it should not outlive the
    /// sandbox. A restarted process asks again, which is right - by then the
    /// runtime may well have been replaced by other means.
    pub(crate) rotation_refused: RwLock<HashSet<SessionId>>,
    /// Who this control plane says it is, on the runtimes it creates.
    ///
    /// A sandbox provider is shared: one key sees everything provisioned
    /// against it, including runtimes belonging to another control plane
    /// entirely. Without an identity on each one, a second control plane reads
    /// every session it has never heard of as an orphan and tears it down.
    pub(crate) identity: Option<String>,
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
    /// Sessions this process is currently bringing up.
    ///
    /// Detached bring-up can outlive its request while no live entry exists;
    /// this distinguishes it from work a reconciliation sweep must recover.
    pub(crate) starting: RwLock<HashSet<SessionId>>,
    /// Milliseconds to wait before the first provisioning retry.
    ///
    /// Held here rather than as a constant so a test can exercise the retry
    /// path without waiting out a delay meant for a provider under load.
    pub(crate) retry_backoff_ms: std::sync::atomic::AtomicU64,
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
    /// Where a session's own credential is minted.
    pub credentials: Option<Arc<dyn CredentialSource>>,
    /// Where a session's skills are fetched from.
    pub skills: Option<Arc<dyn crate::store::SkillSource>>,
    /// The deployment's own settings, when this process has a store for them.
    ///
    /// `None` becomes `unconfigured()`, so sessions still use shipped bounds.
    pub settings: Option<Arc<PlatformSettingsService>>,
}
