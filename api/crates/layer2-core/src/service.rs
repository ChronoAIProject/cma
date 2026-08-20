//! The single tunnel: session lifecycle, the event log, and terminal relay.
//!
//! Every interface calls this and only this. The service owns one live screen
//! model per session and keeps it fed for as long as the runtime produces
//! output — whether or not a client is attached. A detached session that keeps
//! working is the normal case, and a client that reattaches must find the
//! screen already correct rather than replaying history.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use layer3_adapter::{KernelAdapter, RuntimeHandle, TerminalInput};
use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

use crate::authz::{Caller, Capability};
use crate::capabilities::{servable_modes, SessionCapabilities};
use crate::clock::now_rfc3339;
use crate::environment_service::EnvironmentProfileService;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{KernelId, SessionId};
use crate::options::{AgentOptions, SessionMode};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus};
use crate::store::{ContextStore, CredentialSource, SessionRecord, SessionStore, StoreError};
use crate::terminal::{ScreenModel, TerminalGeometry};

/// How many output chunks a slow attached client may fall behind before it is
/// dropped from the live fan-out. It can always recover from a fresh snapshot,
/// so lagging is survivable; unbounded buffering is not.
pub(crate) const LIVE_BUFFER: usize = 512;

pub use crate::lease::{AttachmentHandle, Detached};

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
    /// Where this session's event sequence is handed out.
    ///
    /// Service-wide rather than per-entry, and held here so that numbering an
    /// event needs nothing but the entry already in hand. The alternative —
    /// numbering off `session.last_seq` — is what let a live session and a
    /// provisioning path each advance their own copy and land on one number.
    pub(crate) sequences: Arc<crate::sequence::Sequences>,
    /// Owner-delegated credential last handed to the runtime. Held so a refresh
    /// can be pushed without the caller tracking which sessions exist.
    pub(crate) subject_credential: Option<String>,
    /// The values the session was handed as environment, kept so a tool body
    /// can be redacted of them before it is written. Values only: the core
    /// needs to replace an exact value wherever a tool echoes one, and the key
    /// is not what leaks.
    pub(crate) environment_values: Vec<String>,
    pub(crate) events: Vec<Event>,
    pub(crate) screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    pub(crate) runtime: Option<RuntimeHandle>,
    pub(crate) input: Option<mpsc::Sender<TerminalInput>>,
    /// Where this session's stored context lives, once any has been captured.
    pub(crate) context_ref: Option<String>,
    /// Transcript length at the last capture, so an unchanged one is not rewritten.
    pub(crate) captured_len: usize,
    pub(crate) live: broadcast::Sender<Vec<u8>>,
    /// Liveness on the conversation plane: pieces of a message being assembled.
    ///
    /// Separate from the terminal's byte stream because the two are different
    /// kinds of thing and a session has one plane or the other. Kept typed
    /// rather than as bytes, because a token-level piece of a message is not a
    /// screen and rendering it into one would be a decision the core has no
    /// business making.
    pub(crate) live_frames: broadcast::Sender<crate::event::LiveFrame>,
    /// Input toward the session's conversation, when it holds one.
    pub(crate) conversation: Option<mpsc::Sender<layer3_adapter::ConversationInput>>,
    /// The message the agent is still assembling, and which one it is.
    ///
    /// A runtime streams a message in pieces and says nothing to mark the end
    /// of one; what marks it is the next message starting, or the turn ending.
    /// Held here so the finished message can be committed at that moment
    /// rather than each piece being written down as though it were history.
    pub(crate) assembling: Option<(Option<String>, String)>,
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
    /// is never numbered the same as the one it replaced — which is the whole
    /// basis on which a superseded holder's release is told apart from a real
    /// one.
    pub(crate) attachments_issued: u64,
}

/// What distinguishes one deployment of this service from another.
///
/// Grouped rather than passed alongside the ports because they are the same
/// kind of thing — facts about this installation — and because a constructor
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

/// The core service.
pub struct SessionService {
    /// Live sessions, keyed by id. Only those with a running runtime appear.
    pub(crate) entries: RwLock<HashMap<SessionId, SessionEntry>>,
    /// The single writer of every session's event sequence.
    ///
    /// Shared with each entry, so the conversation fold and the provisioning,
    /// revival, retirement and rotation paths all number from one place. Two
    /// of them numbering independently is what dropped a turn's events.
    pub(crate) sequences: Arc<crate::sequence::Sequences>,
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
    /// sandbox. A restarted process asks again, which is right — by then the
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
    /// The runtime version sessions are expected to be running.
    ///
    /// Held by the control plane rather than asked of each sandbox: this is
    /// the version being deployed, and a session on anything else is one to
    /// carry forward. `None` disables rotation entirely, which is what a
    /// deployment that does not version its runtime image wants.
    pub(crate) runtime_version: Option<String>,
    /// Sessions this process is currently bringing up.
    ///
    /// Bring-up is detached and takes minutes, during which the record says
    /// `provisioning` and no entry exists yet — which is indistinguishable, to
    /// anything reading state, from a bring-up that died with an earlier
    /// process. The controller would then start a second runtime for a session
    /// that is already getting one. This set is the difference between the two.
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
/// of three `Option<Arc<dyn …>>` is one where two can be swapped without the
/// compiler noticing.
#[derive(Default)]
pub struct SessionPorts {
    /// Where a session's conversation is stored between runtimes.
    pub context: Option<Arc<dyn ContextStore>>,
    /// Where a session's own credential is minted.
    pub credentials: Option<Arc<dyn CredentialSource>>,
    /// Where a session's skills are fetched from.
    pub skills: Option<Arc<dyn crate::store::SkillSource>>,
}

impl SessionService {
    /// Build a service over a set of runtime implementations.
    ///
    /// The map is the only place a concrete runtime is named; the core selects
    /// by opaque identifier and never interprets it.
    pub fn new(
        adapters: HashMap<KernelId, Arc<dyn KernelAdapter>>,
        screen_factory: ScreenFactory,
        store: Arc<dyn SessionStore>,
        ports: SessionPorts,
        environments: Arc<EnvironmentProfileService>,
        deployment: Deployment,
    ) -> Arc<Self> {
        let SessionPorts {
            context,
            credentials,
            skills,
        } = ports;
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
            skills,
            sequences: Arc::new(crate::sequence::Sequences::default()),
            starting: RwLock::new(HashSet::new()),
            rotation_refused: RwLock::new(HashSet::new()),
            adapters,
            screen_factory,
            store,
            context,
            credentials,
            environments,
            runtime_version: deployment.runtime_version,
            identity: deployment.identity,
            adopts_untagged: deployment.adopts_untagged,
            retry_backoff_ms: std::sync::atomic::AtomicU64::new(
                crate::provision::DEFAULT_RETRY_BACKOFF_MS,
            ),
        })
    }

    /// Claim a session for bring-up, if nothing else has.
    ///
    /// Returns false when another bring-up already holds it, which is what
    /// stops two concurrent sweeps — or a sweep racing a request — from
    /// provisioning a session twice.
    pub(crate) async fn begin_start(&self, id: &SessionId) -> bool {
        self.starting.write().await.insert(id.clone())
    }

    /// Release a bring-up claim, whatever its outcome.
    pub(crate) async fn finish_start(&self, id: &SessionId) {
        self.starting.write().await.remove(id);
    }

    /// The environment profiles this service resolves sessions against.
    ///
    /// Exposed so a process wiring interfaces up has one service to hand around
    /// rather than two that must be kept consistent.
    pub fn environments(&self) -> &Arc<EnvironmentProfileService> {
        &self.environments
    }

    /// Read a session's durable record.
    pub(crate) async fn record(&self, id: &SessionId) -> Result<SessionRecord, CoreError> {
        self.store
            .get(id)
            .await
            .map_err(store_failed)?
            .ok_or(CoreError::SessionNotFound)
    }

    /// The runtimes this service can back a session with, and what each can be.
    ///
    /// The planes travel with the identifier because a caller choosing a
    /// runtime is choosing what its session will be, and the two questions
    /// answered separately can be answered inconsistently. What each runtime
    /// can serve is read from the adapter rather than configured, so a kernel
    /// that gains or loses an ability is advertised correctly without anything
    /// here being edited.
    pub fn available_kernels(&self) -> Vec<(KernelId, Vec<SessionMode>)> {
        let mut offerings: Vec<_> = self
            .adapters
            .iter()
            .map(|(id, adapter)| (id.clone(), servable_modes(adapter.capabilities())))
            .collect();
        offerings.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        offerings
    }

    /// Create a session and bring its runtime up in the background.
    ///
    /// Returns as soon as the session exists, before any runtime does. Bringing
    /// a runtime up means pulling an image and scheduling a pod, which takes as
    /// long as it takes; holding a request open for it ties the work to a
    /// connection, and when that connection goes away — a timeout, a closed tab
    /// — the work is cancelled halfway, leaving a runtime nothing refers to.
    ///
    /// The record is written before anything is provisioned, so every runtime
    /// that comes to exist is already accounted for. Callers follow the rest
    /// through the event log.
    pub async fn create_session(
        self: &Arc<Self>,
        caller: &Caller,
        spec: SessionSpec,
    ) -> Result<Session, CoreError> {
        caller.authorization.require(Capability::CreateSession)?;
        // Checked before anything is recorded: a selection this caller may not
        // use, or a store that cannot be read, must fail the request rather
        // than leave a session provisioning towards an environment it will
        // never get.
        self.environments
            .resolve(caller, spec.environment_profile.as_ref())
            .await?;

        // The profile says how the agent behaves, so the request does not.
        // Taking it from here rather than from the caller is what makes a
        // session reproducible from the bundle it names: two sessions started
        // on one profile are the same session twice, whoever started them.
        let agent = self
            .environments
            .agent_for(caller, spec.environment_profile.as_ref())
            .await?;
        let mut spec = spec;
        // Which plane a session is on is the exception, and only where no
        // profile was named.
        //
        // Everything else here is how the *agent* behaves — which model, how
        // hard it thinks, how much it may do unattended — and belongs to the
        // profile so that two sessions started on one are the same session
        // twice. A plane is not that. It is how a person means to interact,
        // which is the caller's business and the interface's, not the bundle's.
        //
        // Without this, an interface that exists only to hold conversations
        // could not create one: AG-UI asks for a conversation, the deployment
        // default overwrote it with whatever this deployment happens to prefer,
        // and the very next line refused the session it had just created for
        // lacking the ability it had just asked for. A profile that names a
        // mode still decides, unchanged.
        let mode = match (spec.environment_profile.is_some(), spec.requested_mode) {
            // A profile names one, so it decides. Unchanged.
            (true, _) => agent.mode,
            // No profile, and the caller said which plane they meant.
            (false, Some(asked)) => asked,
            // No profile and nothing asked: this deployment's own default.
            (false, None) => agent.mode,
        };
        spec.options = AgentOptions {
            mode,
            model: agent.model.clone(),
            reasoning_effort: agent.reasoning_effort,
            permission: agent.permission,
            credential: agent.credential,
            record_tool_bodies: agent.record_tool_bodies,
        };

        // Resolved before the record is written: a profile naming a kernel this
        // deployment cannot start is refused outright rather than accepted and
        // then failed asynchronously.
        let adapter =
            self.adapters.get(&agent.kernel).cloned().ok_or_else(|| {
                CoreError::RuntimeUnavailable(format!("no runtime {}", agent.kernel))
            })?;

        // Settled here, while somebody is still on the line to be told. A
        // session whose kernel cannot do what its mode requires can never work,
        // and accepting it would mean writing a record and then failing it
        // asynchronously against a caller who has long since gone.
        let capabilities = SessionCapabilities::granted(spec.options.mode, adapter.capabilities())
            .inspect_err(|refusal| {
                tracing::info!(
                    subject = %caller.subject,
                    kernel = %agent.kernel,
                    reason = %refusal,
                    "refused a session its runtime could not be"
                );
            })?;

        // What the session may reach, settled now and written down. Resolved
        // once here rather than on every later question, because the profile
        // it comes from can be edited afterwards and the session's reach does
        // not follow it.
        let granted_services = self
            .environments
            .services_for(&caller.subject, spec.environment_profile.as_ref())
            .await;

        // Refused here, by name, while somebody is still on the line to be
        // told. A grant can be revoked between saving a profile and starting a
        // session from it, so this is the truthful moment to check — and a
        // session started against a service its owner has taken back would run
        // perfectly and refuse every call the agent made on their behalf,
        // which is the failure this refusal exists to replace.
        self.environments
            .refuse_ungranted(caller, spec.caller_credential.as_deref(), &granted_services)
            .await?;

        let geometry = spec.geometry.unwrap_or_default();
        let id = SessionId::generate();
        let session = Session {
            id: id.clone(),
            owner: caller.subject.clone(),
            kernel: agent.kernel.clone(),
            status: SessionStatus::Provisioning,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            options: spec.options.clone(),
            environment_profile: spec.environment_profile.clone(),
            geometry,
            last_seq: 0,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities,
            granted_services: Some(granted_services),
            created_at: now_rfc3339(),
        };

        let mut record = SessionRecord::new(session.clone());
        // Through the allocator like every other producer, even though nothing
        // can yet be racing a session that did not exist a moment ago: a
        // second place that decides what a sequence is, however safe today, is
        // the shape of the defect this all exists to remove.
        let first = self.sequences.take(&id, 0, 3);
        record.session.last_seq = first + 2;
        self.store.upsert(&record).await.map_err(store_failed)?;
        self.append_durably(
            &id,
            vec![
                Event {
                    session_id: id.clone(),
                    seq: first,
                    kind: EventKind::SessionCreated,
                    appended_at: String::new(),
                },
                Event {
                    session_id: id.clone(),
                    seq: first + 1,
                    kind: EventKind::StatusChanged {
                        status: SessionStatus::Provisioning,
                        stop_reason: None,
                        detail: None,
                        capability: None,
                    },
                    appended_at: String::new(),
                },
                Event {
                    session_id: id.clone(),
                    seq: first + 2,
                    kind: EventKind::Progress {
                        stage: crate::session::ProvisioningStage::Accepted,
                    },
                    appended_at: String::new(),
                },
            ],
        )
        .await
        .map_err(store_failed)?;

        // Detached deliberately. The caller's connection may end at any moment
        // and must not take the runtime with it.
        self.begin_start(&id).await;
        let service = Arc::clone(self);
        let spawned = record.session.clone();
        tokio::spawn(async move {
            service
                .bring_up(
                    spawned,
                    spec,
                    adapter,
                    geometry,
                    None,
                    crate::provision_failure::Abandonment::Retire,
                )
                .await;
        });

        tracing::info!(session_id = %id, kernel = %record.session.kernel, "session created, runtime starting");
        Ok(record.session)
    }

    /// Fail unless the caller owns this live session and it is still running.
    ///
    /// Scoped to live sessions on purpose: this guards the terminal-driving
    /// operations, which are meaningless without a runtime to drive.
    pub(crate) async fn authorize(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        if !entry.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }
        if entry.session.status.is_terminal() {
            return Err(CoreError::SessionTerminated);
        }
        Ok(())
    }

    pub(crate) async fn forward_input(&self, id: &SessionId, input: TerminalInput) {
        let sender = {
            let entries = self.entries.read().await;
            entries.get(id).and_then(|e| e.input.clone())
        };
        if let Some(tx) = sender {
            if tx.send(input).await.is_err() {
                tracing::warn!(session_id = %id, "runtime input channel closed");
            }
        }
    }
}

/// Render a storage failure as a core error.
///
/// Storage being unreachable is reported as the runtime being unavailable
/// rather than as "not found": a session that exists but cannot be read is a
/// different situation from one that does not exist, and conflating them would
/// tell a client their work was gone when it is merely unreachable.
pub(crate) fn store_failed(e: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(format!("session store: {e}"))
}

/// Pump runtime output into the screen model and out to any live subscriber.
pub(crate) fn spawn_relay(
    id: SessionId,
    mut output: mpsc::Receiver<Vec<u8>>,
    screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    live: broadcast::Sender<Vec<u8>>,
) {
    tokio::spawn(async move {
        while let Some(chunk) = output.recv().await {
            screen.lock().await.apply(&chunk);
            // A send with no subscribers is not an error: the session is simply
            // detached, and the screen model has already absorbed the bytes.
            let _ = live.send(chunk);
        }
        tracing::info!(session_id = %id, "runtime output ended");
    });
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
