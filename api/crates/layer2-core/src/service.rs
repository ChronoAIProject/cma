//! The single tunnel: session lifecycle, the event log, and terminal relay.
//!
//! Every interface calls this and only this. The service owns one live screen
//! model per session and keeps it fed for as long as the runtime produces
//! output — whether or not a client is attached. A detached session that keeps
//! working is the normal case, and a client that reattaches must find the
//! screen already correct rather than replaying history.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use layer3_adapter::{KernelAdapter, TerminalInput};
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
            credentials,
            skills,
            settings,
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
            settings: settings.unwrap_or_else(|| Arc::new(PlatformSettingsService::unconfigured())),
            session_limit_guards: Default::default(),
            deployment_capacity_guard: Default::default(),
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
        // Through the allocator like every other producer, even though nothing
        // can yet be racing a session that did not exist a moment ago: a
        // second place that decides what a sequence is, however safe today, is
        // the shape of the defect this all exists to remove.
        let first = self.sequences.take(&id, 0, 3);
        record.session.last_seq = first + 2;
        self.store.upsert(&record).await.map_err(store_failed)?;
        drop(capacity_guard);
        if let Some(error) = settings_fallback.as_ref() {
            tracing::error!(
                session_id = %id,
                error = %error,
                "the session was permanently recorded unpriced because platform settings could not be read"
            );
        }
        limit_guard.record_written(self.store.as_ref()).await;
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

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
