//! The single tunnel: session lifecycle, the event log, and terminal relay.
//!
//! Every interface calls this and only this. The service owns one live screen
//! model per session and keeps it fed for as long as the runtime produces
//! output — whether or not a client is attached. A detached session that keeps
//! working is the normal case, and a client that reattaches must find the
//! screen already correct rather than replaying history.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use layer3_adapter::{AdapterError, KernelAdapter, TerminalInput, TerminalInputSender};
#[cfg(test)]
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::sync::{OwnedMutexGuard, RwLock};

use crate::authz::{Caller, Capability};
use crate::capabilities::{servable_modes, SessionCapabilities};
use crate::clock::now_rfc3339;
use crate::environment::AgentConfiguration;
use crate::environment_service::EnvironmentProfileService;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::ids::{KernelId, SessionId};
use crate::options::{AgentOptions, SessionMode};
use crate::platform::PlatformSettings;
use crate::platform_service::PlatformSettingsService;
use crate::pricing::{format_usd, Quote, SessionPricing, CURRENCY};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus};
use crate::size::SessionSize;
use crate::store::{SessionRecord, SessionStore, StoreError};
#[cfg(test)]
use crate::terminal::ScreenModel;

pub use crate::lease::{AttachmentHandle, Detached};
pub(crate) use crate::service_state::{Attachment, SessionEntry, LIVE_BUFFER, RESOURCE_BUFFER};
pub use crate::service_state::{Deployment, ScreenFactory, SessionPorts, SessionService};

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
            handovers,
            credentials,
            skills,
            workspaces,
            settings,
        } = ports;
        let sequences = Arc::new(crate::sequence::Sequences::default());
        let event_logs = Arc::new(crate::log::EventLogs::new(
            Arc::clone(&store),
            Arc::clone(&sequences),
        ));
        Arc::new(Self {
            entries: RwLock::new(HashMap::new()),
            lifecycle_gates: Default::default(),
            input_gates: Default::default(),
            readoption_gates: Default::default(),
            conversation_order_gates: Default::default(),
            prompt_gates: Default::default(),
            skills,
            workspaces,
            sequences,
            event_logs,
            starting: RwLock::new(HashSet::new()),
            rotation_refused: RwLock::new(HashSet::new()),
            rotations_in_progress: RwLock::new(HashSet::new()),
            adapters,
            screen_factory,
            store,
            context,
            handovers,
            credentials,
            environments,
            settings: settings.unwrap_or_else(|| Arc::new(PlatformSettingsService::unconfigured())),
            session_limit_guards: Default::default(),
            deployment_capacity_guard: Default::default(),
            runtime_version: deployment.runtime_version,
            identity: deployment.identity,
            coordination_holder: format!("cma_{}", uuid::Uuid::new_v4().simple()),
            adopts_untagged: deployment.adopts_untagged,
            retry_backoff_ms: std::sync::atomic::AtomicU64::new(
                crate::provision::DEFAULT_RETRY_BACKOFF_MS,
            ),
            #[cfg(test)]
            pause_next_live_install: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            live_install_paused: tokio::sync::Notify::new(),
            #[cfg(test)]
            live_install_resume: tokio::sync::Notify::new(),
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
        self.rotations_in_progress.write().await.remove(id);
        self.starting.write().await.remove(id);
    }

    pub(crate) async fn rotation_in_progress(&self, id: &SessionId) -> bool {
        self.rotations_in_progress.read().await.contains(id)
    }

    /// Keep legacy provider mutations from bypassing durable coordination.
    ///
    /// Compatibility stores and generation-zero sessions still use the old
    /// lifecycle while the coordinated implementations are rolled out. Once a
    /// non-zero authority exists, every provider mutation must arrive through a
    /// lease-aware path; callers without one fail closed instead of falling back
    /// to a handle-only adapter call. The remaining compatibility path has no
    /// cross-replica object identity of its own and is limited to providers that
    /// either capture immutable deletion identity inside the adapter or do not
    /// reuse a handle while a lifecycle operation is in flight.
    pub(crate) async fn require_compatibility_provider_mutation(
        &self,
        id: &SessionId,
        operation: &str,
    ) -> Result<(), CoreError> {
        match self.store.runtime_authority(id).await {
            Ok(Some(authority)) if authority.generation != 0 => {
                Err(CoreError::RuntimeUnavailable(format!(
                    "{operation} requires a durable session lease for coordinated runtime authority"
                )))
            }
            Ok(Some(_)) | Err(StoreError::CoordinationDisabled) => Ok(()),
            Ok(None) => Err(CoreError::SessionNotFound),
            Err(error) => Err(store_failed(error)),
        }
    }

    #[cfg(test)]
    pub(crate) fn pause_next_live_install(&self) {
        assert!(
            !self
                .pause_next_live_install
                .swap(true, std::sync::atomic::Ordering::SeqCst),
            "a live installation pause is already armed"
        );
    }

    #[cfg(test)]
    pub(crate) async fn wait_until_live_install_paused(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.live_install_paused.notified(),
        )
        .await
        .expect("readiness was committed before the replacement entry was installed");
    }

    #[cfg(test)]
    pub(crate) fn resume_live_install(&self) {
        self.live_install_resume.notify_one();
    }

    #[cfg(test)]
    pub(crate) async fn pause_live_install_if_armed(&self) {
        if self
            .pause_next_live_install
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.live_install_paused.notify_one();
            self.live_install_resume.notified().await;
        }
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

    /// Quote the size a new session will actually receive.
    ///
    /// `None` means this deployment has configured no price. A configured
    /// price without a resolved size is refused rather than guessed: Layer 2
    /// has no numeric representation of the deployment's opaque reservation.
    pub async fn quote(
        &self,
        caller: &Caller,
        profile: Option<&crate::ids::EnvironmentProfileId>,
    ) -> Result<Option<Quote>, CoreError> {
        caller.authorization.require(Capability::CreateSession)?;
        let agent = self.environments.agent_for(caller, profile).await?;
        let size = Self::resolve_size(&agent);
        let settings = self.settings.for_provisioning().await;
        Self::quote_from(&settings, size)
    }

    /// The one size resolution used by both quoting and creation.
    ///
    /// An authored profile size is the only typed size that exists today.
    /// `None` remains the deployment's opaque reservation; this function does
    /// not parse provider quantities or manufacture a default.
    fn resolve_size(agent: &AgentConfiguration) -> Option<SessionSize> {
        agent.size
    }

    fn quote_from(
        settings: &PlatformSettings,
        size: Option<SessionSize>,
    ) -> Result<Option<Quote>, CoreError> {
        let Some(card) = settings.pricing.in_force() else {
            return Ok(None);
        };
        let Some(size) = size else {
            return Err(CoreError::InvalidRequest(
                "this priced session has no resolved size; an administrator must declare the deployment's assumed session size"
                    .into(),
            ));
        };
        let cpu_millis = size.cpu_millis();
        let memory_mib = size.memory_mib();
        let micros_per_hour = card
            .rate
            .micros_per_hour(cpu_millis, memory_mib)
            .ok_or_else(|| {
                CoreError::InvalidRequest(format!(
                    "the configured rate for {cpu_millis}m CPU and {memory_mib} MiB exceeds the largest supported hourly price"
                ))
            })?;
        Ok(Some(Quote {
            rate_card: card.id.clone(),
            rate: card.rate.clone(),
            cpu_millis,
            memory_mib,
            micros_per_hour,
            display: format_usd(micros_per_hour),
            currency: CURRENCY,
            quoted_at: now_rfc3339(),
        }))
    }

    /// Fail unless this deployment has declared room for one more session.
    ///
    /// Requiring a caller keeps the check on an asking path. Reconciliation and
    /// runtime replacement restore promises already recorded and never call it.
    /// The returned guard is held through the durable record write so two local
    /// creates cannot both spend the same headroom.
    async fn refuse_at_deployment_capacity(
        &self,
        caller: &Caller,
        declared: crate::DeploymentCapacity,
        requested: Option<SessionSize>,
    ) -> Result<Option<OwnedMutexGuard<()>>, CoreError> {
        if !declared.is_declared() {
            return Ok(None);
        }

        let held = Arc::clone(&self.deployment_capacity_guard)
            .lock_owned()
            .await;
        let mut reserved = self.store.reserved_capacity().await.map_err(store_failed)?;
        let requested = match requested.or(declared.assumed_session_size) {
            Some(size) => size,
            None => {
                tracing::warn!(
                    subject = %caller.subject,
                    unsized_sessions = reserved.unsized_sessions.saturating_add(1),
                    "deployment capacity is declared but no cost is declared for unsized sessions; skipping the capacity check"
                );
                return Ok(Some(held));
            }
        };

        if reserved.unsized_sessions > 0 {
            let Some(assumed) = declared.assumed_session_size else {
                tracing::warn!(
                    subject = %caller.subject,
                    unsized_sessions = reserved.unsized_sessions,
                    "deployment capacity is declared but no cost is declared for unsized sessions; skipping the capacity check"
                );
                return Ok(Some(held));
            };
            reserved = reserved.assuming(assumed);
        }

        if let Err(shortfall) = declared.check(reserved, requested) {
            tracing::info!(
                subject = %caller.subject,
                requested_cpu_millis = shortfall.requested_cpu_millis,
                requested_memory_mib = shortfall.requested_memory_mib,
                free_cpu_millis = shortfall.free_cpu_millis,
                free_memory_mib = shortfall.free_memory_mib,
                "refused a session larger than the deployment's declared room"
            );
            return Err(CoreError::DeploymentAtCapacity {
                requested_cpu_millis: shortfall.requested_cpu_millis,
                requested_memory_mib: shortfall.requested_memory_mib,
                free_cpu_millis: shortfall.free_cpu_millis,
                free_memory_mib: shortfall.free_memory_mib,
            });
        }

        Ok(Some(held))
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
        self.create_session_with_identity(caller, spec, SessionId::generate(), None)
            .await
    }

    async fn create_session_with_identity(
        self: &Arc<Self>,
        caller: &Caller,
        spec: SessionSpec,
        id: SessionId,
        trigger_origin: Option<crate::TriggerSessionOrigin>,
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
        let size = Self::resolve_size(&agent);
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
            size,
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

        let (settings, settings_fallback) = self.settings.for_provisioning_observed().await;
        if settings_fallback.is_none() {
            if let Some(quoted) = spec.quoted_rate_card.as_ref() {
                let in_force = settings.pricing.in_force().map(|card| &card.id);
                if in_force != Some(quoted) {
                    return Err(CoreError::PriceChanged {
                        quoted: quoted.0.clone(),
                        // A legitimate quote can only move to another card:
                        // sealed history cannot become empty. This spelling
                        // still gives a fabricated claim against an unpriced
                        // deployment an actionable re-quote response.
                        in_force: in_force
                            .map(|id| id.0.clone())
                            .unwrap_or_else(|| "unpriced".into()),
                    });
                }
            }
        }
        let pricing = Self::quote_from(&settings, size)?.map(|quote| SessionPricing {
            rate_card: quote.rate_card,
            rate: quote.rate,
            cpu_millis: quote.cpu_millis,
            memory_mib: quote.memory_mib,
            micros_per_hour: quote.micros_per_hour,
            quoted_at: quote.quoted_at,
        });
        if let Some(size) = size {
            settings.size_bounds.check(size)?;
        }
        let capacity_guard = self
            .refuse_at_deployment_capacity(caller, settings.deployment_capacity, size)
            .await?;
        let limit_guard = self
            .refuse_at_session_limit(caller, settings.live_sessions)
            .await?;

        let geometry = spec.geometry.unwrap_or_default();
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
            /*
             * Not known yet, and deliberately not guessed.
             *
             * `agent.skills` here is the configuration's own list, before the
             * baseline's mandated entries are folded in and before the
             * registry has been asked — so writing it would record a set that
             * is neither what was requested nor what arrives. Provisioning is
             * the only place that knows which packs were actually fetched, and
             * it is the only writer of this field.
             *
             * `None` until then says "not recorded", which is true for the
             * seconds a session spends provisioning and stays true for one
             * that never finished.
             */
            granted_skills: None,
            pricing,
            created_at: now_rfc3339(),
        };

        let mut record = SessionRecord::new(session.clone());
        record.trigger_origin = trigger_origin;
        if let Err(error) = self
            .create_durably(
                &mut record,
                vec![
                    Event {
                        session_id: id.clone(),
                        seq: 0,
                        kind: EventKind::SessionCreated,
                        appended_at: String::new(),
                    },
                    Event {
                        session_id: id.clone(),
                        seq: 0,
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
                        seq: 0,
                        kind: EventKind::Progress {
                            stage: crate::session::ProvisioningStage::Accepted,
                        },
                        appended_at: String::new(),
                    },
                ],
            )
            .await
        {
            // This identifier was generated for this request and never became
            // a session the caller can use. Keeping its three reserved event
            // positions would retain one strong map key per failed creation.
            self.sequences.forget(&id);
            return Err(store_failed(error));
        }
        drop(capacity_guard);
        if let Some(error) = settings_fallback.as_ref() {
            tracing::error!(
                session_id = %id,
                error = %error,
                "the session was permanently recorded unpriced because platform settings could not be read"
            );
        }
        limit_guard.record_written(self.store.as_ref()).await;

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
                    crate::provision_support::BringUpContext::compatibility(
                        crate::provision_failure::Abandonment::Retire,
                        None,
                    ),
                )
                .await;
        });

        tracing::info!(session_id = %id, kernel = %record.session.kernel, "session created, runtime starting");
        Ok(record.session)
    }

    /// Ensure the one deterministic Codex session owned by a Trigger Launch.
    ///
    /// The durable Launch is the authority. No browser credential is retained
    /// or forwarded: credential custody resolves the owner's current token when
    /// detached provisioning mints the session-scoped NyxID key.
    pub async fn ensure_trigger_session(
        self: &Arc<Self>,
        launch: &crate::Launch,
    ) -> Result<Session, CoreError> {
        let connection = launch.connection.as_ref().ok_or_else(|| {
            CoreError::InvalidRequest("the Launch GitHub connection is not frozen".into())
        })?;
        let origin = trigger_session_origin(launch)?;
        if launch.effective.permission == crate::PermissionMode::Unrestricted {
            return Err(CoreError::InvalidRequest(
                "unrestricted permission is not allowed for a Trigger session".into(),
            ));
        }

        let mut agent = self.environments.default_agent().clone();
        agent.kernel = KernelId::new("codex");
        agent.mode = SessionMode::Conversation;
        agent.permission = launch.effective.permission;
        agent.credential = crate::CredentialMode::Delegated;
        agent.size = Some(launch.effective.resource_size);
        agent.record_tool_bodies = false;
        let profile_spec = crate::EnvironmentProfileSpec {
            name: format!("CMA Trigger {}", launch.id),
            agent,
            variables: Vec::new(),
            packages: Vec::new(),
            services: vec![crate::ServiceId::new(connection.service_id.clone())],
            agent_notes: String::new(),
        };
        let profile = self
            .environments
            .ensure_trigger_profile(&launch.owner, &origin, profile_spec)
            .await?;
        let id = origin
            .session_id()
            .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
        let expected_services = self
            .environments
            .services_for(&launch.owner, Some(&profile.id))
            .await;
        let expected_options = AgentOptions {
            mode: profile.agent.mode,
            model: profile.agent.model.clone(),
            reasoning_effort: profile.agent.reasoning_effort,
            permission: profile.agent.permission,
            credential: profile.agent.credential,
            size: profile.agent.size,
            record_tool_bodies: profile.agent.record_tool_bodies,
        };

        // One process cannot race itself. Mongo's create-only transaction is
        // the cross-process fence; a losing replica rereads and verifies below.
        let creation = self.lock_lifecycle(&id).await;
        if let Some(record) = self.store.get(&id).await.map_err(store_failed)? {
            let session = verify_trigger_session_record(
                &record,
                &origin,
                &launch.owner,
                &profile.id,
                &expected_options,
                &expected_services,
            )?;
            drop(creation);
            return Ok(session);
        }

        let caller = Caller::new(
            launch.owner.clone(),
            crate::Authorization::from_capabilities([
                Capability::CreateSession,
                Capability::ManageOwnEnvironmentProfiles,
            ]),
        );
        let spec = SessionSpec {
            options: AgentOptions::default(),
            geometry: None,
            environment_profile: Some(profile.id.clone()),
            caller_credential: None,
            runtime_credential: None,
            requested_mode: Some(SessionMode::Conversation),
            quoted_rate_card: None,
        };
        let attempted = self
            .create_session_with_identity(&caller, spec, id.clone(), Some(origin.clone()))
            .await;
        drop(creation);
        match attempted {
            Ok(session) => Ok(session),
            Err(original) => match self.store.get(&id).await {
                Ok(Some(record)) => verify_trigger_session_record(
                    &record,
                    &origin,
                    &launch.owner,
                    &profile.id,
                    &expected_options,
                    &expected_services,
                ),
                _ => Err(original),
            },
        }
    }

    /// Whether the current runtime acknowledged its session-scoped credential.
    pub async fn trigger_credential_ready(&self, id: &SessionId) -> bool {
        self.entries
            .read()
            .await
            .get(id)
            .is_some_and(|entry| entry.subject_credential.is_some())
    }

    /// Fail unless the caller owns this live session and it is still running.
    ///
    /// Scoped to live sessions on purpose: this guards the terminal-driving
    /// operations, which are meaningless without a runtime to drive.
    pub(crate) async fn authorize(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
        self.session_for_interaction(caller, id).await?;
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(id) {
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            if entry.session.status.is_terminal() {
                return Err(CoreError::SessionTerminated);
            }
            // `is_interactive` also excludes provisioning and idle sessions, whose
            // existing missing-runtime answers remain unchanged. Rotation alone
            // still holds an outgoing channel that must not receive new input.
            if entry.session.status.is_rotating() {
                return Err(CoreError::SessionRotating);
            }
            return Ok(());
        }
        drop(entries);
        let rotation_in_progress = self.rotation_in_progress(id).await;
        Err(self
            .missing_live_session_error(caller, id, rotation_in_progress)
            .await)
    }

    /// Classify an operation whose live runtime entry disappeared.
    ///
    /// Compatibility handover can lack a live entry while provisioning its
    /// replacement. The durable record identifies most of that interval; the
    /// in-process marker spans the final boundary where readiness is durable but
    /// the replacement entry is not installed yet. Both are consulted only after
    /// the live lookup misses so ordinary terminal input retains its in-memory
    /// path. Ownership is checked before lifecycle state so this fallback cannot
    /// disclose another owner's session.
    pub(crate) async fn missing_live_session_error(
        &self,
        caller: &Caller,
        id: &SessionId,
        rotation_in_progress: bool,
    ) -> CoreError {
        let record = match self.record(id).await {
            Ok(record) => record,
            Err(error) => return error,
        };
        if !record.session.is_owned_by(&caller.subject) {
            return CoreError::Forbidden;
        }
        if record.session.status.is_terminal() {
            return CoreError::SessionTerminated;
        }
        if record.is_effectively_rotating() || rotation_in_progress {
            return CoreError::SessionRotating;
        }
        CoreError::SessionNotFound
    }

    pub(crate) async fn forward_input(
        &self,
        id: &SessionId,
        sender: &TerminalInputSender,
        input: TerminalInput,
    ) -> Result<(), CoreError> {
        sender.send(input).await.map_err(|error| {
            tracing::warn!(session_id = %id, %error, "terminal input was not delivered");
            match error {
                AdapterError::Unavailable(detail) => CoreError::RuntimeUnavailable(detail),
                AdapterError::Rejected(detail)
                | AdapterError::AtCapacity(detail)
                | AdapterError::Unsupported(detail) => CoreError::RuntimeRefused(detail),
            }
        })
    }
}

pub(crate) fn trigger_session_origin(
    launch: &crate::Launch,
) -> Result<crate::TriggerSessionOrigin, CoreError> {
    let workspace = launch.workspace.as_ref().ok_or_else(|| {
        CoreError::InvalidRequest("the Launch workspace has not been imported".into())
    })?;
    workspace
        .baseline
        .validate()
        .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
    let connection = launch.connection.as_ref().ok_or_else(|| {
        CoreError::InvalidRequest("the Launch GitHub connection is not frozen".into())
    })?;
    let origin = crate::TriggerSessionOrigin {
        launch_id: launch.id.clone(),
        revision_digest: launch.revision_digest.clone(),
        sandbox_profile: launch.revision.content.session.sandbox_profile.clone(),
        github_service_id: connection.service_id.clone(),
        workspace_epoch: workspace.workspace_epoch,
        source_revision: launch.source_sha.clone(),
        manifest_digest: workspace.baseline.digest.clone(),
        object_reference: workspace.baseline_object_reference.clone(),
        archive_sha256: workspace.baseline_archive_sha256.clone(),
    };
    origin
        .validate()
        .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
    Ok(origin)
}

fn verify_trigger_session_record(
    record: &SessionRecord,
    origin: &crate::TriggerSessionOrigin,
    owner: &crate::SubjectId,
    profile_id: &crate::EnvironmentProfileId,
    expected_options: &AgentOptions,
    expected_services: &[crate::ServiceId],
) -> Result<Session, CoreError> {
    let session = &record.session;
    let exact = record.trigger_origin.as_ref() == Some(origin)
        && session.id
            == origin
                .session_id()
                .map_err(|error| CoreError::InvalidRequest(error.to_string()))?
        && &session.owner == owner
        && session.environment_profile.as_ref() == Some(profile_id)
        && session.kernel == KernelId::new("codex")
        && &session.options == expected_options
        && session.capabilities.conversation
        && session.granted_services.as_deref() == Some(expected_services);
    if !exact {
        return Err(CoreError::InvalidRequest(
            "the deterministic Trigger session identity is occupied by different authority or configuration"
                .into(),
        ));
    }
    Ok(session.clone())
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

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
