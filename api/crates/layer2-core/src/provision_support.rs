//! Coordination and record helpers for bringing a session runtime up.

use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{
    KernelAdapter, RuntimeCapacity, RuntimeHandle, RuntimeOptions, RuntimePermission,
};

use crate::capabilities::SessionCapabilities;
use crate::event::{Event, EventKind};
use crate::ids::SessionId;
use crate::options::{AgentOptions, PermissionMode};
use crate::service::SessionService;
use crate::session::{ProvisioningStage, Session, SessionSpec};
use crate::size::SessionSize;
use crate::terminal::TerminalGeometry;

/// How long a runtime may take to become usable.
///
/// Generous, because a cold image pull onto a fresh node is not fast and giving
/// up early would fail sessions that were about to work. Bounded all the same:
/// without a deadline a provider that never answers leaves a session waiting
/// for something that is not coming, and the sandbox it may already have
/// created is never reclaimed.
const PROVISION_DEADLINE: Duration = Duration::from_secs(600);

impl SessionService {
    /// Provision a runtime for an existing session and attach its terminal.
    pub(crate) async fn bring_up(
        self: &Arc<Self>,
        session: Session,
        spec: SessionSpec,
        adapter: Arc<dyn KernelAdapter>,
        geometry: TerminalGeometry,
        restore: Option<Vec<u8>>,
        outcome: crate::provision_failure::Abandonment,
    ) {
        let id = session.id.clone();
        /*
         * The session's own credential, minted against what it was granted.
         *
         * Unconditional, and that is the fix for a defect this gate used to
         * hide. It read `if the caller supplied one, use theirs` - and an
         * interface did supply one: the caller's own access token, put on the
         * spec so the platform could be asked questions as them. Provisioning
         * found the field filled, skipped the mint, and handed the agent its
         * owner's whole identity. Every session a person created ran that way,
         * while the minting machinery ran only for sessions the controller
         * recovered, which is the one path nobody takes by hand.
         *
         * `credential_for` is idempotent per session: asking twice - for a
         * replaced runtime, or after a restart - returns the value already
         * issued rather than leaving two alive for one session.
         */
        let mut spec = spec;
        let services = self.services_for(&session.id).await;
        let reaches: Vec<String> = services.iter().map(|service| service.0.clone()).collect();
        // Render here, where both inputs are authoritative: this service owns
        // the deployment setting and the session record owns its frozen grant.
        // Layer 3 receives prose and has no reason to understand either model.
        let platform_briefing = self
            .settings
            .for_provisioning()
            .await
            .briefing
            .render(&reaches);
        spec.runtime_credential = self
            .credential_for(&session.id, &session.owner, &services)
            .await;
        if spec.runtime_credential.is_none() {
            tracing::warn!(
                session_id = %id,
                "no credential could be minted for this session; it will start unable to act \
                 on its owner's behalf until the top-up sweep supplies one"
            );
        }
        match tokio::time::timeout(
            PROVISION_DEADLINE,
            self.start_with_retries(
                &session,
                &spec,
                &adapter,
                geometry,
                restore.as_ref(),
                &platform_briefing,
            ),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(failure)) => {
                if let Err(error) = self.abandon(&id, &adapter, &failure, outcome).await {
                    tracing::error!(
                        session_id = %id,
                        %error,
                        "cannot record the runtime abandonment; leaving it intact"
                    );
                }
            }
            Err(_) => {
                if let Err(error) = self
                    .abandon(
                        &id,
                        &adapter,
                        &crate::provision_failure::StartFailure::transient(format!(
                            "the runtime did not become usable within {}s",
                            PROVISION_DEADLINE.as_secs()
                        )),
                        outcome,
                    )
                    .await
                {
                    tracing::error!(
                        session_id = %id,
                        %error,
                        "cannot record the timed-out runtime abandonment; leaving it intact"
                    );
                }
            }
        }
        // Released last, and on every path: while the claim is held the
        // controller leaves this session alone, so failing to release it would
        // strand a session nothing is allowed to recover.
        self.finish_start(&id).await;
    }

    /// Append a progress step to the session's log.
    ///
    /// Written durably rather than pushed at whoever happens to be watching,
    /// so someone who opens the page mid-start still sees where it got to. A
    /// step that cannot be recorded is logged and retained for the next
    /// session transaction: progress is commentary, and failing a session
    /// because its commentary failed would be absurd.
    pub(crate) async fn note_progress(&self, id: &SessionId, stage: ProvisioningStage) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            return;
        };
        if let Err(error) = self
            .persist_durably(
                &mut record,
                vec![Event {
                    session_id: id.clone(),
                    seq: 0,
                    kind: EventKind::Progress { stage },
                    appended_at: String::new(),
                }],
            )
            .await
        {
            tracing::warn!(session_id = %id, error = %error, "cannot record progress");
        }
    }

    /// Record the runtime a session was given, as soon as it exists.
    /// Write down the skills this runtime was actually given.
    ///
    /// Not the set the session asked for. A rotation has no caller to fetch as
    /// and falls back to this service's own identity, which sees only public
    /// skills - so a session can legitimately come back with fewer than it
    /// started with, and #442 is about stopping that. Until it does, the record
    /// must say what is on the disk rather than what was once requested.
    ///
    /// The difference is not cosmetic: the record is what every interface
    /// reports, so a record describing skills the sandbox does not have is an
    /// interface telling somebody their agent can do something it cannot.
    pub(crate) async fn remember_skills(&self, id: &SessionId, skills: &[crate::SkillPack]) {
        let installed: Vec<crate::environment::GrantedSkill> = skills
            .iter()
            .map(|pack| crate::environment::GrantedSkill {
                id: crate::environment::SkillId::new(pack.name.clone()),
                version: pack.version.clone(),
            })
            .collect();
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(session_id = %id, "cannot record the skills: session is gone");
            return;
        };
        if record.session.granted_skills.as_deref() == Some(installed.as_slice()) {
            return;
        }
        tracing::info!(
            session_id = %id,
            had = ?record.session.granted_skills,
            now = ?installed,
            "recording the skills this runtime was actually given"
        );
        record.session.granted_skills = Some(installed);
        if let Err(e) = self.persist_durably(&mut record, Vec::new()).await {
            tracing::error!(
                session_id = %id,
                error = %e,
                "skills installed but not recorded; the record now overstates what this session has"
            );
        }
    }

    pub(crate) async fn remember_runtime(&self, id: &SessionId, handle: &RuntimeHandle) {
        let Ok(Some(mut record)) = self.store.get(id).await else {
            tracing::error!(session_id = %id, "cannot record the runtime: session is gone");
            return;
        };
        record.runtime_id = Some(handle.0.clone());
        // Stamped with what it was actually started on, so the next sweep can
        // tell a session that is current from one that is behind. Without this
        // every session would look due for rotation forever.
        record.session.runtime_version = self.runtime_version.clone();
        if let Err(e) = self.persist_durably(&mut record, Vec::new()).await {
            // Worth shouting about: the runtime exists and the record does not
            // name it, which is exactly the orphan reconciliation must catch.
            tracing::error!(session_id = %id, error = %e, "runtime provisioned but not recorded");
        }
    }

    /// Give a session abilities that were never recorded, and record them.
    ///
    /// Two jobs that must not be separated. It refuses a session whose runtime
    /// can no longer be what the session is - the check every start makes -
    /// and it writes the answer down where the session was recorded before
    /// this model existed.
    ///
    /// The second job is why this exists. `granted` is the only thing that
    /// produces a set, and until now the two paths that bring a session *back*
    /// — readopting a surviving runtime, and starting a replacement — either
    /// copied the record verbatim or computed the value and discarded it. A
    /// session written by an older build therefore came back holding nothing,
    /// which is not "granted nothing": it is refused at every gate that asks,
    /// including the one behind every keystroke.
    ///
    /// This also settles a single field added after the set existed. Filling a
    /// field that was never written does not change a recorded grant: there is
    /// no earlier answer to overwrite. Every field that was recorded remains
    /// untouched.
    ///
    /// Recorded rather than recomputed per process, because the record is the
    /// authority and a set that exists only in memory is one the next control
    /// plane has to derive again.
    pub(crate) async fn settle_capabilities(
        &self,
        session: &Session,
        adapter: &Arc<dyn KernelAdapter>,
    ) -> Result<SessionCapabilities, crate::error::CoreError> {
        let granted = SessionCapabilities::granted(session.options.mode, adapter.capabilities())?;
        if !session.capabilities.unrecorded() && session.capabilities.resources.is_some() {
            return Ok(session.capabilities);
        }
        let fallback = if session.capabilities.unrecorded() {
            granted
        } else {
            SessionCapabilities {
                resources: granted.resources,
                ..session.capabilities
            }
        };
        match self.store.get(&session.id).await {
            Ok(Some(mut record)) => {
                let current = record.session.capabilities;
                let (settled, resources_only) = if current.unrecorded() {
                    (granted, false)
                } else if current.resources.is_none() {
                    (
                        SessionCapabilities {
                            resources: granted.resources,
                            ..current
                        },
                        true,
                    )
                } else {
                    return Ok(current);
                };
                record.session.capabilities = settled;
                if let Err(e) = self.persist_durably(&mut record, Vec::new()).await {
                    // Worth saying and not worth refusing over: the session
                    // works with the set in hand, and the next sweep settles
                    // it again.
                    tracing::warn!(session_id = %session.id, error = %e, "cannot record what this session holds");
                } else if resources_only {
                    tracing::info!(
                        session_id = %session.id,
                        "settled resource reporting; this ability was never recorded"
                    );
                } else {
                    tracing::info!(
                        session_id = %session.id,
                        "settled what this session holds; it was recorded before capabilities existed"
                    );
                }
                Ok(settled)
            }
            Ok(None) => Ok(fallback),
            Err(e) => {
                tracing::warn!(session_id = %session.id, error = %e, "cannot read this session to settle what it holds");
                Ok(fallback)
            }
        }
    }
}

/// Translate what a person asked for into what a runtime is told.
///
/// The two vocabularies are separate on purpose - the core must not speak any
/// runtime's dialect - so this is the single place they meet, and the only
/// place a new option has to be taught to cross.
pub(crate) fn runtime_options(options: &AgentOptions) -> RuntimeOptions {
    RuntimeOptions {
        mode: match options.mode {
            crate::options::SessionMode::Terminal => layer3_adapter::RuntimeMode::Terminal,
            crate::options::SessionMode::Conversation => layer3_adapter::RuntimeMode::Conversation,
        },
        model: options.model.clone(),
        reasoning_effort: Some(options.reasoning_effort.as_str().to_string()),
        permission: match options.permission {
            PermissionMode::Guarded => RuntimePermission::Guarded,
            PermissionMode::Autonomous => RuntimePermission::Autonomous,
            PermissionMode::Unrestricted => RuntimePermission::Unrestricted,
        },
        own_credential: matches!(options.credential, crate::options::CredentialMode::Own),
        // Nothing to rejoin: this is a runtime being started, which has no
        // conversation yet. Taking one back over is the readopt path's, and it
        // reads the name from the record rather than from anything held here.
        resume_conversation: None,
    }
}

/// Translate the size a person asked for into what a runtime is given.
///
/// Beside `runtime_options` and for the same reason: the two vocabularies are
/// separate on purpose, so this is the one place they meet. `None` crosses as
/// `None` - the caller states nothing and the adapter shapes the runtime the
/// way this deployment shapes every runtime.
pub(crate) fn runtime_capacity(size: Option<SessionSize>) -> Option<RuntimeCapacity> {
    size.map(|size| RuntimeCapacity {
        cpu_millis: size.cpu_millis,
        memory_mib: size.memory_mib,
    })
}
