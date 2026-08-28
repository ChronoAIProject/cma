//! The single tunnel: session lifecycle, the event log, and terminal relay.
//!
//! Every interface calls this and only this. The service owns one live screen
//! model per session and keeps it fed for as long as the runtime produces
//! output — whether or not a client is attached. A detached session that keeps
//! working is the normal case, and a client that reattaches must find the
//! screen already correct rather than replaying history.

use std::collections::{BTreeSet, HashMap, HashSet};
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
use crate::pricing::{estimate_micros, format_usd, Quote, SessionPricing, CURRENCY};
use crate::session::{RecoveryState, Session, SessionSpec, SessionStatus};
use crate::size::SessionSize;
use crate::store::{SessionRecord, SessionStore, StoreError};
#[cfg(test)]
use crate::terminal::ScreenModel;

pub use crate::lease::{AttachmentHandle, Detached};
pub(crate) use crate::service_state::{Attachment, SessionEntry, LIVE_BUFFER, RESOURCE_BUFFER};
pub use crate::service_state::{Deployment, ScreenFactory, SessionPorts, SessionService};

/// Whether this creation mints an identity or preserves one derived elsewhere.
enum CreationIdentity {
    Fresh,
    Exact(SessionId),
}

impl CreationIdentity {
    fn resolve_with(self, generate_fresh: impl FnOnce() -> SessionId) -> SessionId {
        match self {
            Self::Fresh => generate_fresh(),
            Self::Exact(id) => id,
        }
    }
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
            handovers,
            credentials,
            skills,
            workspaces,
            skill_packs,
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
            skill_packs,
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
            trigger_runtime_capabilities: deployment.trigger_runtime_capabilities,
            trigger_kernel: deployment.trigger_kernel,
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
            .filter(|(id, _)| self.trigger_kernel.as_ref() != Some(*id))
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
        Self::quote_from_at(settings, size, now_rfc3339())
    }

    fn quote_from_at(
        settings: &PlatformSettings,
        size: Option<SessionSize>,
        quoted_at: String,
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
            quoted_at,
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
        self.create_session_with_identity(
            caller,
            spec,
            CreationIdentity::Fresh,
            None,
            SessionId::generate,
        )
        .await
    }

    async fn create_session_with_identity<F>(
        self: &Arc<Self>,
        caller: &Caller,
        spec: SessionSpec,
        identity: CreationIdentity,
        trigger_origin: Option<crate::TriggerSessionOrigin>,
        generate_fresh_id: F,
    ) -> Result<Session, CoreError>
    where
        F: FnOnce() -> SessionId + Send,
    {
        caller.authorization.require(Capability::CreateSession)?;
        // Checked before anything is recorded: a selection this caller may not
        // use, or a store that cannot be read, must fail the request rather
        // than leave a session provisioning towards an environment it will
        // never get.
        let resolved_environment = self
            .environments
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
        if trigger_origin.is_none() && self.trigger_kernel.as_ref() == Some(&agent.kernel) {
            return Err(CoreError::InvalidRequest(format!(
                "runtime {} is reserved for managed Trigger sessions",
                agent.kernel
            )));
        }
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

        // Resolved while the caller's delegated authority still exists, then
        // staged in the same atomic create as the grant derived from these exact
        // bytes. Reconciliation therefore never needs to recreate a private
        // answer from a request credential that disappeared with this call.
        let skill_packs = self
            .resolve_skill_packs_for_creation(
                spec.caller_credential.as_deref(),
                &resolved_environment.skills,
            )
            .await?;
        crate::store::validate_skill_packs(&skill_packs)
            .map_err(CoreError::SkillCatalogUnavailable)?;
        if !skill_packs.is_empty() && self.skill_packs.is_none() {
            return Err(CoreError::RuntimeUnavailable(
                "durable skill pack storage is not configured; refusing to create a session whose skills could not be recovered or erased"
                    .into(),
            ));
        }
        if let Some(store) = self.skill_packs.as_ref() {
            store.validate(&skill_packs).map_err(store_failed)?;
        }
        let granted_skills = Self::granted_skills_for(&skill_packs);

        let (settings, settings_fallback) = self.settings.for_provisioning_observed().await;
        if trigger_origin.is_none() && settings_fallback.is_none() {
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
        let pricing = match trigger_origin.as_ref() {
            Some(origin) => origin.pricing.clone(),
            None => Self::quote_from(&settings, size)?.map(|quote| SessionPricing {
                rate_card: quote.rate_card,
                rate: quote.rate,
                cpu_millis: quote.cpu_millis,
                memory_mib: quote.memory_mib,
                micros_per_hour: quote.micros_per_hour,
                quoted_at: quote.quoted_at,
            }),
        };
        if let Some(size) = size {
            settings.size_bounds.check(size)?;
        }
        let capacity_guard = self
            .refuse_at_deployment_capacity(caller, settings.deployment_capacity, size)
            .await?;
        let limit_guard = self
            .refuse_at_session_limit(caller, settings.live_sessions)
            .await?;

        let id = identity.resolve_with(generate_fresh_id);
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
            granted_skills: Some(granted_skills),
            pricing,
            created_at: now_rfc3339(),
        };

        let mut record = SessionRecord::new(session.clone());
        record.trigger_origin = trigger_origin;
        record.skill_packs = crate::store::SkillPackState::Staged(skill_packs);
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
            // This identifier never became a session the caller can use.
            // Keeping its three reserved event positions would retain one
            // strong map key per failed creation.
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
        let trigger_origin = record.trigger_origin.clone();
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
                    )
                    .with_trigger_origin(trigger_origin),
                )
                .await;
        });

        tracing::info!(session_id = %id, kernel = %record.session.kernel, "session created, runtime starting");
        Ok(record.session)
    }

    /// Ensure the one deterministic managed-agent session owned by a Trigger Launch.
    ///
    /// The durable Launch is the authority. No browser credential is retained
    /// or forwarded: credential custody resolves the owner's current token when
    /// detached provisioning mints the session-scoped NyxID key.
    pub async fn ensure_trigger_session(
        self: &Arc<Self>,
        launch: &crate::Launch,
    ) -> Result<Session, CoreError> {
        let provisioning_at = now_rfc3339();
        self.revalidate_trigger_provisioning_admission(launch, &provisioning_at)
            .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
        let sandbox_profile =
            crate::TriggerSandboxProfile::parse(&launch.revision.content.session.sandbox_profile)
                .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
        let connection = launch.connection.as_ref().ok_or_else(|| {
            CoreError::InvalidRequest("the Launch GitHub connection is not frozen".into())
        })?;
        let origin = trigger_session_origin(launch)?;
        let exact_service = crate::ServiceId::new(origin.github_service_id.clone());
        let credentials = self.credentials.as_ref().ok_or_else(|| {
            CoreError::ServiceCatalogUnavailable(
                "no credential source can verify the selected Trigger service".into(),
            )
        })?;
        match credentials
            .exact_services_available(&launch.owner, std::slice::from_ref(&exact_service))
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(CoreError::ServiceNotGranted(exact_service.0)),
            Err(error) => {
                tracing::warn!(
                    launch_id = %launch.id,
                    error = %error,
                    "could not verify the selected Trigger service before session creation"
                );
                return Err(CoreError::ServiceCatalogUnavailable(
                    "the selected Trigger service could not be verified".into(),
                ));
            }
        }
        if launch.effective.permission == crate::PermissionMode::Unrestricted {
            return Err(CoreError::InvalidRequest(
                "unrestricted permission is not allowed for a Trigger session".into(),
            ));
        }

        let trigger_kernel = self.trigger_kernel.as_ref().ok_or_else(|| {
            CoreError::RuntimeUnavailable("the Trigger runtime kernel is not configured".into())
        })?;
        let agent = sandbox_profile.resolve_agent(
            self.environments.default_agent(),
            trigger_kernel,
            launch.effective.permission,
            launch.effective.resource_size,
        );
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
                &profile.agent.kernel,
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
            .create_session_with_identity(
                &caller,
                spec,
                CreationIdentity::Exact(id.clone()),
                Some(origin.clone()),
                SessionId::generate,
            )
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
                    &profile.agent.kernel,
                    &expected_options,
                    &expected_services,
                ),
                _ => Err(original),
            },
        }
    }

    async fn trigger_review_admission(
        &self,
        owner: &crate::SubjectId,
        launch: &crate::Launch,
        now: &str,
    ) -> Result<crate::LaunchReviewAdmission, crate::TriggerAdmissionError> {
        launch
            .require_live_draft(now)
            .map_err(|error| crate::TriggerAdmissionError::Unavailable(error.to_string()))?;
        let settings = self
            .settings
            .for_trigger_admission()
            .await
            .map_err(|error| crate::TriggerAdmissionError::Unavailable(error.to_string()))?;
        settings
            .size_bounds
            .check(launch.effective.resource_size)
            .map_err(|error| crate::TriggerAdmissionError::QuotaExceeded(error.to_string()))?;
        let pricing = self.trigger_pricing_snapshot(launch, &settings, now)?;
        let runtime = self.trigger_runtime_snapshot(launch, now)?;
        let quota = self
            .trigger_quota_snapshot(owner, launch.effective.resource_size, &settings, now)
            .await?;
        Ok(crate::LaunchReviewAdmission {
            pricing,
            runtime,
            quota,
        })
    }

    async fn revalidate_trigger_start_admission(
        &self,
        launch: &crate::Launch,
        now: &str,
    ) -> Result<crate::StartAdmissionSnapshot, crate::TriggerAdmissionError> {
        let reviewed_pricing = launch.pricing.as_ref().ok_or_else(|| {
            crate::TriggerAdmissionError::PricingUnavailable(
                "the Launch has no reviewed pricing snapshot".into(),
            )
        })?;
        let reviewed_runtime = launch.runtime_admission.as_ref().ok_or_else(|| {
            crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: "runtime_admission".into(),
            }
        })?;
        let now_seconds = crate::seconds_from_rfc3339(now).ok_or_else(|| {
            crate::TriggerAdmissionError::Unavailable("the server clock is malformed".into())
        })?;
        let quote_expiry =
            crate::seconds_from_rfc3339(&reviewed_pricing.expires_at).ok_or_else(|| {
                crate::TriggerAdmissionError::PricingUnavailable(
                    "the reviewed quote expiry is malformed".into(),
                )
            })?;
        if now_seconds >= quote_expiry {
            return Err(crate::TriggerAdmissionError::QuoteExpired);
        }

        let settings = self
            .settings
            .for_trigger_admission()
            .await
            .map_err(|error| crate::TriggerAdmissionError::Unavailable(error.to_string()))?;
        let current_pricing = self.trigger_pricing_snapshot(launch, &settings, now)?;
        if current_pricing.policy_reference != reviewed_pricing.policy_reference
            || current_pricing.billing_mode != reviewed_pricing.billing_mode
            || current_pricing.rate_card != reviewed_pricing.rate_card
            || current_pricing.rate != reviewed_pricing.rate
            || current_pricing.micros_per_hour != reviewed_pricing.micros_per_hour
            || current_pricing.estimated_micros != reviewed_pricing.estimated_micros
        {
            return Err(crate::TriggerAdmissionError::PriceChanged);
        }
        let current_runtime = self.trigger_runtime_snapshot(launch, now)?;
        if current_runtime.runtime_version != reviewed_runtime.runtime_version
            || current_runtime.kernel != reviewed_runtime.kernel
        {
            return Err(crate::TriggerAdmissionError::RuntimeChanged);
        }
        let quota = self
            .trigger_quota_snapshot(
                &launch.owner,
                launch.effective.resource_size,
                &settings,
                now,
            )
            .await?;
        Ok(crate::StartAdmissionSnapshot {
            runtime_version: current_runtime.runtime_version,
            pricing_policy_reference: reviewed_pricing.policy_reference.clone(),
            rate_card: reviewed_pricing.rate_card.clone(),
            quota,
            checked_at: now.to_string(),
        })
    }

    /// Recheck facts that may change between Start and sandbox provisioning.
    ///
    /// Start already accepted and pinned the pricing promise. A retired rate
    /// card or elapsed draft deadline therefore cannot invalidate later
    /// provisioning. Runtime identity and required capabilities remain live
    /// safety facts and are deliberately checked again here.
    fn revalidate_trigger_provisioning_admission(
        &self,
        launch: &crate::Launch,
        now: &str,
    ) -> Result<(), crate::TriggerAdmissionError> {
        let reviewed_pricing = launch.pricing.as_ref().ok_or_else(|| {
            crate::TriggerAdmissionError::PricingUnavailable(
                "the Launch has no pinned pricing snapshot".into(),
            )
        })?;
        let reviewed_runtime = launch.runtime_admission.as_ref().ok_or_else(|| {
            crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: "runtime_admission".into(),
            }
        })?;
        let start = launch.start_admission.as_ref().ok_or_else(|| {
            crate::TriggerAdmissionError::Unavailable(
                "the Launch has no successful Start admission".into(),
            )
        })?;
        self.validate_trigger_pinned_pricing(launch, reviewed_pricing, start)?;

        let current_runtime = self.trigger_runtime_snapshot(launch, now)?;
        if current_runtime.runtime_version != reviewed_runtime.runtime_version
            || current_runtime.kernel != reviewed_runtime.kernel
            || current_runtime.runtime_version != start.runtime_version
        {
            return Err(crate::TriggerAdmissionError::RuntimeChanged);
        }
        Ok(())
    }

    fn validate_trigger_pinned_pricing(
        &self,
        launch: &crate::Launch,
        pricing: &crate::PricingSnapshot,
        start: &crate::StartAdmissionSnapshot,
    ) -> Result<(), crate::TriggerAdmissionError> {
        let policy = &launch.revision.content.commercial_policy;
        let size = launch.effective.resource_size;
        let estimated = estimate_micros(pricing.micros_per_hour, launch.effective.timeout_seconds)
            .ok_or_else(|| {
                crate::TriggerAdmissionError::PricingUnavailable(
                    "the pinned maximum session estimate exceeds the supported ledger".into(),
                )
            })?;
        let common_is_valid = pricing.policy_reference == policy.policy_reference
            && pricing.billing_mode == policy.billing_mode
            && pricing.currency == CURRENCY
            && pricing.cpu_millis == size.cpu_millis()
            && pricing.memory_mib == size.memory_mib()
            && crate::seconds_from_rfc3339(&pricing.quoted_at).is_some()
            && crate::seconds_from_rfc3339(&pricing.expires_at).is_some()
            && pricing.quoted_at < pricing.expires_at
            && pricing.estimated_micros == estimated
            && start.pricing_policy_reference == pricing.policy_reference
            && start.rate_card == pricing.rate_card;
        let terms_are_valid = match pricing.billing_mode {
            crate::BillingMode::ZeroCost => {
                pricing.policy_reference == crate::CMA_TRIGGER_ZERO_COST_POLICY_V1
                    && pricing.rate_card.is_none()
                    && pricing.rate.is_none()
                    && pricing.micros_per_hour == 0
                    && pricing.estimated_micros == 0
            }
            crate::BillingMode::Contributor => {
                pricing.policy_reference == crate::CMA_TRIGGER_CONTRIBUTOR_POLICY_V1
                    && pricing.rate_card.is_some()
                    && pricing.rate.as_ref().is_some_and(|rate| {
                        rate.micros_per_hour(pricing.cpu_millis, pricing.memory_mib)
                            == Some(pricing.micros_per_hour)
                    })
                    && pricing.micros_per_hour > 0
            }
            crate::BillingMode::Sponsored => false,
        };
        if !common_is_valid || !terms_are_valid {
            return Err(crate::TriggerAdmissionError::PricingUnavailable(
                "the pinned pricing promise is incomplete or inconsistent".into(),
            ));
        }
        Ok(())
    }

    fn trigger_pricing_snapshot(
        &self,
        launch: &crate::Launch,
        settings: &PlatformSettings,
        now: &str,
    ) -> Result<crate::PricingSnapshot, crate::TriggerAdmissionError> {
        let policy = &launch.revision.content.commercial_policy;
        let size = launch.effective.resource_size;
        match policy.billing_mode {
            crate::BillingMode::ZeroCost => {
                if policy.policy_reference != crate::CMA_TRIGGER_ZERO_COST_POLICY_V1 {
                    return Err(crate::TriggerAdmissionError::PricingUnavailable(
                        "the zero-cost policy reference is not deployment-owned".into(),
                    ));
                }
                Ok(crate::PricingSnapshot {
                    policy_reference: policy.policy_reference.clone(),
                    billing_mode: policy.billing_mode,
                    rate_card: None,
                    rate: None,
                    currency: CURRENCY.into(),
                    cpu_millis: size.cpu_millis(),
                    memory_mib: size.memory_mib(),
                    micros_per_hour: 0,
                    estimated_micros: 0,
                    quoted_at: now.to_string(),
                    expires_at: launch.draft_expires_at.clone(),
                })
            }
            crate::BillingMode::Contributor => {
                if policy.policy_reference != crate::CMA_TRIGGER_CONTRIBUTOR_POLICY_V1 {
                    return Err(crate::TriggerAdmissionError::PricingUnavailable(
                        "the contributor policy reference is not deployment-owned".into(),
                    ));
                }
                let quote = Self::trigger_contributor_quote(settings, size, now)?;
                let estimated_micros =
                    estimate_micros(quote.micros_per_hour, launch.effective.timeout_seconds)
                        .ok_or_else(|| {
                            crate::TriggerAdmissionError::PricingUnavailable(
                                "the maximum session estimate exceeds the supported ledger".into(),
                            )
                        })?;
                Ok(crate::PricingSnapshot {
                    policy_reference: policy.policy_reference.clone(),
                    billing_mode: policy.billing_mode,
                    rate_card: Some(quote.rate_card),
                    rate: Some(quote.rate),
                    currency: quote.currency.into(),
                    cpu_millis: quote.cpu_millis,
                    memory_mib: quote.memory_mib,
                    micros_per_hour: quote.micros_per_hour,
                    estimated_micros,
                    quoted_at: quote.quoted_at,
                    expires_at: launch.draft_expires_at.clone(),
                })
            }
            crate::BillingMode::Sponsored => Err(crate::TriggerAdmissionError::PricingUnavailable(
                "sponsored billing is disabled for the public MVP".into(),
            )),
        }
    }

    fn trigger_contributor_quote(
        settings: &PlatformSettings,
        size: SessionSize,
        now: &str,
    ) -> Result<Quote, crate::TriggerAdmissionError> {
        let quote = Self::quote_from_at(settings, Some(size), now.to_string())
            .map_err(|error| crate::TriggerAdmissionError::PricingUnavailable(error.to_string()))?
            .ok_or_else(|| {
                crate::TriggerAdmissionError::PricingUnavailable(
                    "contributor billing requires an in-force platform rate card".into(),
                )
            })?;
        if quote.micros_per_hour == 0 {
            return Err(crate::TriggerAdmissionError::PricingUnavailable(
                "the contributor rate must produce a positive hourly amount".into(),
            ));
        }
        Ok(quote)
    }

    async fn trigger_preview_cost(
        &self,
        revision: &crate::TriggerRevision,
        now: &str,
    ) -> crate::TriggerPreviewCost {
        let policy = &revision.content.commercial_policy;
        if policy.billing_mode != crate::BillingMode::Contributor
            || policy.policy_reference != crate::CMA_TRIGGER_CONTRIBUTOR_POLICY_V1
        {
            return crate::TriggerPreviewCost::QuoteUnavailable;
        }
        let settings = match self.settings.for_trigger_admission().await {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(
                    public_trigger_id = %revision.public_id,
                    %error,
                    "the public Trigger preview cannot read deployment pricing"
                );
                return crate::TriggerPreviewCost::QuoteUnavailable;
            }
        };
        let quote = match Self::trigger_contributor_quote(
            &settings,
            revision.content.session.resources.default,
            now,
        ) {
            Ok(quote) => quote,
            Err(error) => {
                tracing::debug!(
                    public_trigger_id = %revision.public_id,
                    %error,
                    "the public Trigger preview has no contributor quote"
                );
                return crate::TriggerPreviewCost::QuoteUnavailable;
            }
        };
        let Some(amount_micros) = estimate_micros(
            quote.micros_per_hour,
            revision.content.session.timeout_default_seconds,
        ) else {
            tracing::warn!(
                public_trigger_id = %revision.public_id,
                "the public Trigger estimate exceeds the supported ledger"
            );
            return crate::TriggerPreviewCost::QuoteUnavailable;
        };
        crate::TriggerPreviewCost::ContributorEstimate {
            currency: quote.currency.into(),
            amount_micros,
            display: format_usd(amount_micros),
            quoted_at: quote.quoted_at,
        }
    }

    fn trigger_runtime_snapshot(
        &self,
        launch: &crate::Launch,
        now: &str,
    ) -> Result<crate::RuntimeAdmissionSnapshot, crate::TriggerAdmissionError> {
        let runtime_version = self
            .runtime_version
            .as_ref()
            .filter(|version| !version.trim().is_empty())
            .cloned()
            .ok_or_else(|| crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: "pinned_runtime_version".into(),
            })?;
        let trigger_kernel = self.trigger_kernel.as_ref().ok_or_else(|| {
            crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: "trigger_kernel".into(),
            }
        })?;
        if trigger_kernel.is_unset() {
            return Err(crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: "trigger_kernel".into(),
            });
        }
        let adapter = self.adapters.get(trigger_kernel).ok_or_else(|| {
            crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: "trigger_kernel".into(),
            }
        })?;
        let kernel = adapter.capabilities();
        let mut supported: BTreeSet<_> =
            self.trigger_runtime_capabilities.iter().copied().collect();
        if !kernel.conversation {
            supported.remove(&crate::RequiredCapability::Conversation);
        }
        if !kernel.conversation || !kernel.snapshot {
            supported.remove(&crate::RequiredCapability::DurableConversation);
        }
        if self.workspaces.is_none() {
            supported.remove(&crate::RequiredCapability::WorkspaceCheckpoint);
        }
        if self.credentials.is_none() {
            supported.remove(&crate::RequiredCapability::RestrictedServiceGateway);
            supported.remove(&crate::RequiredCapability::CredentialRefresh);
            supported.remove(&crate::RequiredCapability::CredentialRevocation);
        }
        if let Some(missing) = launch
            .revision
            .content
            .session
            .required_capabilities
            .iter()
            .find(|capability| !supported.contains(capability))
        {
            return Err(crate::TriggerAdmissionError::CapabilityUnsupported {
                capability: trigger_capability_name(*missing).into(),
            });
        }
        Ok(crate::RuntimeAdmissionSnapshot {
            kernel: trigger_kernel.clone(),
            runtime_version,
            supported_capabilities: supported.into_iter().collect(),
            observed_at: now.to_string(),
        })
    }

    async fn trigger_quota_snapshot(
        &self,
        owner: &crate::SubjectId,
        requested: SessionSize,
        settings: &PlatformSettings,
        now: &str,
    ) -> Result<crate::QuotaSnapshot, crate::TriggerAdmissionError> {
        let live = self.store.count_live_for(owner).await.map_err(|error| {
            crate::TriggerAdmissionError::Unavailable(format!(
                "the live-session ledger could not be read: {error}"
            ))
        })?;
        let limit = settings.live_sessions.map(std::num::NonZeroU32::get);
        if limit.is_some_and(|limit| live >= limit) {
            return Err(crate::TriggerAdmissionError::QuotaExceeded(format!(
                "{live} live sessions already consume the limit of {}",
                limit.unwrap()
            )));
        }
        let mut reserved = self.store.reserved_capacity().await.map_err(|error| {
            crate::TriggerAdmissionError::Unavailable(format!(
                "the deployment-capacity ledger could not be read: {error}"
            ))
        })?;
        let declared = settings.deployment_capacity;
        if reserved.unsized_sessions > 0 {
            let assumed = declared.assumed_session_size.ok_or_else(|| {
                crate::TriggerAdmissionError::Unavailable(
                    "deployment capacity cannot price existing unsized sessions".into(),
                )
            })?;
            reserved = reserved.assuming(assumed);
        }
        let (free_cpu_millis, free_memory_mib) = if declared.is_declared() {
            let room = declared.check(reserved, requested).map_err(|shortfall| {
                crate::TriggerAdmissionError::QuotaExceeded(format!(
                    "requested {}m/{}MiB but only {}m/{}MiB is declared free",
                    shortfall.requested_cpu_millis,
                    shortfall.requested_memory_mib,
                    shortfall.free_cpu_millis,
                    shortfall.free_memory_mib
                ))
            })?;
            (
                declared.cpu_millis.map(|_| room.free_cpu_millis),
                declared.memory_mib.map(|_| room.free_memory_mib),
            )
        } else {
            (None, None)
        };
        Ok(crate::QuotaSnapshot {
            live_sessions: live,
            live_session_limit: limit,
            reserved_cpu_millis: reserved.cpu_millis,
            reserved_memory_mib: reserved.memory_mib,
            free_cpu_millis,
            free_memory_mib,
            checked_at: now.to_string(),
        })
    }

    /// Whether the current runtime acknowledged its session-scoped credential.
    pub async fn trigger_credential_ready(&self, id: &SessionId) -> bool {
        self.entries
            .read()
            .await
            .get(id)
            .is_some_and(|entry| entry.subject_credential.is_some())
    }

    /// Exact live runtime generation for a Trigger session.
    ///
    /// A generation is returned only when the process-local entry and durable
    /// runtime authority name the same handle and generation. Compatibility
    /// stores have no separate authority document; their lifecycle-fenced
    /// Trigger entry carries the monotonic `rotations + 1` generation.
    pub async fn trigger_runtime_generation(
        &self,
        id: &SessionId,
    ) -> Result<Option<u64>, StoreError> {
        let entry = {
            let entries = self.entries.read().await;
            let Some(entry) = entries.get(id) else {
                return Ok(None);
            };
            let (Some(origin), Some(runtime)) =
                (entry.trigger_origin.as_ref(), entry.runtime.as_ref())
            else {
                return Ok(None);
            };
            let expected_session = origin
                .session_id()
                .map_err(|error| StoreError::Malformed(error.to_string()))?;
            if expected_session != *id || entry.runtime_generation == 0 {
                return Ok(None);
            }
            (runtime.0.clone(), entry.runtime_generation)
        };
        match self.store.runtime_authority(id).await {
            Ok(Some(authority)) => Ok((authority.generation == entry.1
                && authority.runtime_id.as_deref() == Some(entry.0.as_str()))
            .then_some(entry.1)),
            Err(StoreError::CoordinationDisabled) => Ok(Some(entry.1)),
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Exact current credential authority, only after the live Trigger runtime
    /// acknowledged credential delivery.
    pub async fn trigger_credential_authority(
        &self,
        id: &SessionId,
    ) -> Result<Option<crate::SessionCredentialAuthority>, StoreError> {
        let acknowledged_trigger_runtime = self.entries.read().await.get(id).is_some_and(|entry| {
            entry.trigger_origin.is_some()
                && entry.runtime.is_some()
                && entry.subject_credential.is_some()
        });
        if !acknowledged_trigger_runtime {
            return Ok(None);
        }
        let Some(source) = self.credentials.as_ref() else {
            return Ok(None);
        };
        let Some(authority) = source.credential_authority(id).await? else {
            return Ok(None);
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let now = u64::try_from(now).unwrap_or(u64::MAX);
        Ok((authority.session_id == *id && authority.is_valid_at(now)).then_some(authority))
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

#[async_trait::async_trait]
impl crate::TriggerLaunchAdmission for SessionService {
    async fn preview_cost(
        &self,
        revision: &crate::TriggerRevision,
        now: &str,
    ) -> crate::TriggerPreviewCost {
        self.trigger_preview_cost(revision, now).await
    }

    async fn review(
        &self,
        owner: &crate::SubjectId,
        launch: &crate::Launch,
        now: &str,
    ) -> Result<crate::LaunchReviewAdmission, crate::TriggerAdmissionError> {
        self.trigger_review_admission(owner, launch, now).await
    }

    async fn start(
        &self,
        owner: &crate::SubjectId,
        launch: &crate::Launch,
        now: &str,
    ) -> Result<crate::StartAdmissionSnapshot, crate::TriggerAdmissionError> {
        if owner != &launch.owner {
            return Err(crate::TriggerAdmissionError::Unavailable(
                "the Launch owner changed during admission".into(),
            ));
        }
        self.revalidate_trigger_start_admission(launch, now).await
    }
}

fn trigger_capability_name(capability: crate::RequiredCapability) -> &'static str {
    match capability {
        crate::RequiredCapability::Conversation => "conversation",
        crate::RequiredCapability::DurableConversation => "durable_conversation",
        crate::RequiredCapability::WorkspaceCheckpoint => "workspace_checkpoint",
        crate::RequiredCapability::RestrictedServiceGateway => "restricted_service_gateway",
        crate::RequiredCapability::CredentialRefresh => "credential_refresh",
        crate::RequiredCapability::CredentialRevocation => "credential_revocation",
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
    let (github_catalog_service_id, github_user_service_slug) = connection
        .github_binding()
        .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
    let pricing = launch.pricing.as_ref().ok_or_else(|| {
        CoreError::InvalidRequest("the Launch pricing snapshot is not frozen".into())
    })?;
    let session_pricing = match pricing.billing_mode {
        crate::BillingMode::ZeroCost => None,
        crate::BillingMode::Contributor => Some(SessionPricing {
            rate_card: pricing.rate_card.clone().ok_or_else(|| {
                CoreError::InvalidRequest("the contributor rate card is missing".into())
            })?,
            rate: pricing.rate.clone().ok_or_else(|| {
                CoreError::InvalidRequest("the contributor rate is missing".into())
            })?,
            cpu_millis: pricing.cpu_millis,
            memory_mib: pricing.memory_mib,
            micros_per_hour: pricing.micros_per_hour,
            quoted_at: pricing.quoted_at.clone(),
        }),
        crate::BillingMode::Sponsored => {
            return Err(CoreError::InvalidRequest(
                "sponsored Trigger billing is disabled".into(),
            ));
        }
    };
    let origin = crate::TriggerSessionOrigin {
        launch_id: launch.id.clone(),
        revision_digest: launch.revision_digest.clone(),
        sandbox_profile: launch.revision.content.session.sandbox_profile.clone(),
        github_service_id: connection.service_id.clone(),
        github_catalog_service_id: Some(github_catalog_service_id.to_string()),
        github_user_service_slug: Some(github_user_service_slug.to_string()),
        workspace_epoch: workspace.workspace_epoch,
        source_revision: launch.source_sha.clone(),
        manifest_digest: workspace.baseline.digest.clone(),
        object_reference: workspace.baseline_object_reference.clone(),
        archive_sha256: workspace.baseline_archive_sha256.clone(),
        timeout_seconds: launch.effective.timeout_seconds,
        pricing_policy_reference: pricing.policy_reference.clone(),
        billing_mode: pricing.billing_mode,
        pricing: session_pricing,
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
    expected_kernel: &KernelId,
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
        && &session.kernel == expected_kernel
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
