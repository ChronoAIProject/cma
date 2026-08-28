//! Bringing a session's runtime up, after the session already exists.
//!
//! This runs detached from whoever asked for the session, because provisioning
//! takes as long as pulling an image and scheduling a pod takes, and tying that
//! to a request means a closed connection cancels it halfway.
//!
//! Detaching it introduces the failure this module is mostly about. Nobody is
//! waiting to be told it went wrong, so a failure that is merely logged leaves
//! a session sitting in `provisioning` forever — which is worse than the
//! synchronous version, where at least the caller saw the error. Every path out
//! of here therefore ends in a recorded state.

use std::sync::Arc;

use layer3_adapter::ConversationUpdate;
use layer3_adapter::{KernelAdapter, RuntimeSpec, TermSize, TerminalInput};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, watch, Mutex};

/// How many updates may queue between a kernel and the folder.
///
/// The folder writes to the durable log, so it is slower than a kernel that is
/// streaming; this is the slack that keeps a fast turn from blocking on it.
pub(crate) const CONVERSATION_BUFFER: usize = 64;

use crate::coordination::{SessionBoundary, SessionLease};
use crate::event::{Event, EventKind};
use crate::provision_failure::StartFailure;
pub(crate) use crate::provision_support::{runtime_capacity, runtime_options};
use crate::service::{SessionEntry, SessionService, LIVE_BUFFER, RESOURCE_BUFFER};
use crate::session::{ProvisioningStage, RecoveryState, Session, SessionSpec, SessionStatus};
use crate::terminal::{ScreenModel, TerminalGeometry};
use crate::terminal_io::spawn_relay;
use crate::WorkspaceWriterHoldReceipt;

/// How many times to try bringing a runtime up before giving up.
///
/// Provisioning fails for reasons that are nobody's decision and usually pass:
/// a provider under load, a node being replaced, a pull that took longer than
/// the call allowed. Someone asking for a session wants a session, and a
/// transient failure is not an answer to give them — so the first failure is
/// retried rather than reported.
pub(crate) const PROVISION_ATTEMPTS: u32 = 4;

/// Wait before the first retry, doubling thereafter.
///
/// Long enough to let a provider under load recover, short enough that a
/// session which succeeds on the second attempt still feels like it started
/// rather than like it stalled.
pub(crate) const DEFAULT_RETRY_BACKOFF_MS: u64 = 3_000;

/// Immutable inputs shared by every attempt to start one runtime.
#[derive(Clone, Copy)]
pub(crate) struct RuntimeStartContext<'a> {
    pub(crate) geometry: TerminalGeometry,
    pub(crate) restore: Option<&'a Vec<u8>>,
    pub(crate) trigger_recovery: Option<&'a crate::TriggerRecoveryPair>,
    pub(crate) platform_briefing: &'a str,
    pub(crate) handover: Option<&'a crate::rotate::CoordinatedHandover>,
    pub(crate) publication_token: &'a str,
}

impl SessionService {
    /// What becomes of the session if this cannot be done.
    ///
    /// Taken as a parameter rather than decided here, because from inside a
    /// bring-up a session being created and a session being carried onto a
    /// replacement runtime look identical — and the right answer is opposite.
    ///
    /// The successful path, from nothing to an attached terminal.
    pub(crate) async fn start_runtime(
        self: &Arc<Self>,
        session: &Session,
        spec: &SessionSpec,
        adapter: &Arc<dyn KernelAdapter>,
        context: RuntimeStartContext<'_>,
        lease: &mut Option<SessionLease>,
        prepared_skills: &mut Option<crate::provision_support::PreparedSkillPacks>,
    ) -> Result<(), StartFailure> {
        let RuntimeStartContext {
            geometry,
            restore,
            trigger_recovery,
            platform_briefing,
            handover,
            publication_token,
        } = context;
        let id = session.id.clone();
        // Checked again, because this also runs for a session being revived or
        // carried onto a replacement runtime — long after it was created, and
        // possibly onto a build of its kernel that no longer does what the
        // session was granted. Starting it anyway would leave a session whose
        // recorded abilities and real ones disagree, which is the state every
        // caller downstream trusts cannot happen.
        //
        // Settled rather than merely checked: a session recorded before this
        // model existed holds nothing, and nothing is a set that refuses every
        // operation it is consulted for. The record is read back below, so
        // writing it here is what the entry is then built from.
        self.settle_capabilities(session, adapter)
            .await
            .map_err(|refusal| {
                let named = match &refusal {
                    crate::error::CoreError::CapabilityUnsupported(what) => Some(what.clone()),
                    _ => None,
                };
                let failure = StartFailure::permanent(format!(
                    "this session's runtime can no longer be what it is: {refusal}"
                ));
                // The refusal arrived typed and knew which ability was
                // missing. Keeping only the sentence is where that was lost.
                match named {
                    Some(what) => failure.lacking(what),
                    None => failure,
                }
            })?;
        // Resolved from the session rather than the request, so a runtime
        // replaced long afterwards is built the same way. A profile that has
        // since been deleted or changed hands fails the attempt rather than
        // silently starting a session without the tools it is meant to have.
        let environment = self
            .environments
            .resolve_for_owner(&session.owner, session.environment_profile.as_ref())
            .await
            .map_err(|e| {
                StartFailure::transient(format!(
                    "this session's environment could not be prepared: {e}"
                ))
            })?;
        let Some(prepared) = self
            .prepare_skill_packs(
                session,
                &environment.skills,
                prepared_skills,
                true,
                lease.as_ref(),
            )
            .await?
        else {
            return Ok(());
        };
        let skills = &prepared.packs;
        self.note_progress(&id, ProvisioningStage::RuntimeRequested)
            .await;
        // What the runtime is told, translated once. Some kernels settle how
        // much the agent may do unattended per conversation rather than per
        // runtime, so this is needed twice and computing it twice would be two
        // places for the translation to drift.
        let mut options = runtime_options(&spec.options);
        /*
         * Read from the session rather than from the profile, exactly as the
         * credential's own scope is: a session keeps what it was granted when
         * it was provisioned, and a bundle widened or narrowed since must not
         * change what a running session believes it may reach.
         *
         * Names only. The credential minted against them travels a different
         * path entirely and never enters the sandbox.
         */
        let reaches: Vec<String> = self
            .services_for(&id)
            .await
            .into_iter()
            .map(|service| service.0)
            .collect();
        let durable = self.store.get(&id).await.map_err(|error| {
            StartFailure::transient(format!(
                "this session's runtime authority could not be read: {error}"
            ))
        })?;
        let trigger_origin = durable
            .as_ref()
            .and_then(|record| record.trigger_origin.as_ref());
        let runtime_policy_required = trigger_origin.is_some();
        options.restricted_service_gateway = runtime_policy_required;
        let trigger_timeout_seconds =
            trigger_origin.map(|origin| u64::from(origin.timeout_seconds));
        let (workspace, restore) = match trigger_recovery {
            Some(pair) => {
                let origin = trigger_origin.ok_or_else(|| {
                    StartFailure::permanent(
                        "a non-Trigger session cannot consume a Trigger recovery pair".into(),
                    )
                })?;
                pair.validate_for(origin).map_err(|error| {
                    StartFailure::permanent(format!(
                        "this Trigger session's recovery pair failed integrity verification: {error}"
                    ))
                })?;
                (
                    Some(pair.workspace.clone()),
                    Some(pair.conversation.clone()),
                )
            }
            None => (self.workspace_for(&id).await?, restore.cloned()),
        };
        // Kept for redaction: a tool body must not carry a value the session
        // was handed, and these are exactly those values.
        let environment_values: Vec<String> = environment.variables.values().cloned().collect();
        if let Some(size) = spec.options.size {
            tracing::info!(
                session_id = %id,
                cpu_millis = size.cpu_millis(),
                memory_mib = size.memory_mib(),
                "provisioning at this session's own size"
            );
        }
        self.renew_runtime_lease(lease.as_ref())
            .await
            .map_err(|error| {
                StartFailure::transient(format!(
                    "runtime authority expired before provider create: {error}"
                ))
            })?;
        // New for every provider-create attempt. A failed attempt may already
        // be visible at the provider, so a retry must reserve a later
        // generation rather than reusing an identity recovery may have seen.
        let reservation = match lease.as_ref() {
            Some(held) => {
                let token = format!("rres_{}", uuid::Uuid::new_v4().simple());
                Some(
                    self.store
                        .reserve_runtime_generation(held, &token)
                        .await
                        .map_err(|error| {
                            StartFailure::transient(format!(
                                "a runtime generation could not be reserved: {error}"
                            ))
                        })?,
                )
            }
            None => None,
        };
        let handle = adapter
            .provision(&RuntimeSpec {
                session_id: id.0.clone(),
                generation: reservation.as_ref().map(|reserved| reserved.generation),
                reservation_token: reservation.as_ref().map(|reserved| reserved.token.clone()),
                size: TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                },
                capacity: runtime_capacity(spec.options.size),
                timeout_seconds: trigger_timeout_seconds,
                // The provider request predates the exact runtime target, so a
                // credential selected here could be superseded by a late H1
                // acknowledgement. Delivery happens after authority switches.
                subject_credential: None,
                options: options.clone(),
                restore_from: restore,
                workspace,
                skills: skills
                    .iter()
                    .map(|pack| layer3_adapter::SkillPackage {
                        name: pack.name.clone(),
                        files: pack.files.clone(),
                    })
                    .collect(),
                platform_briefing: platform_briefing.to_string(),
                reaches,
                runtime_policy_required,
                environment: environment.into(),
                created_by: self.identity.clone(),
            })
            .await
            .map_err(StartFailure::from_provisioning)?;

        let candidate = match (lease.as_ref(), reservation.as_ref()) {
            (Some(held), Some(reserved)) => {
                self.renew_runtime_lease(Some(held))
                    .await
                    .map_err(|error| {
                        StartFailure::transient(format!(
                            "runtime authority expired before candidate registration: {error}"
                        ))
                    })?;
                Some(
                    self.store
                        .register_runtime_candidate(held, reserved, &handle.0)
                        .await
                        .map_err(|error| {
                            StartFailure::transient(format!(
                                "the provisioned runtime could not be registered durably: {error}"
                            ))
                        })?,
                )
            }
            (None, None) => None,
            _ => {
                return Err(StartFailure::transient(
                    "runtime reservation state did not match its operation lease".into(),
                ))
            }
        };
        let runtime_generation = candidate.as_ref().map_or_else(
            || {
                if runtime_policy_required {
                    u64::from(session.rotations).saturating_add(1)
                } else {
                    0
                }
            },
            |candidate| candidate.generation.get(),
        );

        // Compatibility stores still name the handle immediately. Coordinated
        // stores keep it as uncommitted-candidate cleanup debt until attach
        // succeeds and the store switches authority atomically.
        if lease.is_none() {
            self.remember_runtime(&id, &handle).await;
        }
        self.note_progress(&id, ProvisioningStage::RuntimeCreated)
            .await;
        // The skills are on the pod's disk by now: the adapter puts them there
        // before it returns, in the window before the agent starts. Said as its
        // own step because it is time a person waits through that is neither the
        // provider nor the agent.
        if !skills.is_empty() {
            self.note_progress(&id, ProvisioningStage::SkillsInstalled)
                .await;
        }
        self.note_progress(&id, ProvisioningStage::Attaching).await;

        // Whichever plane this session was granted, and only that one. A
        // terminal session opens no conversation and a conversation session
        // attaches no terminal: the two are exclusive by construction, and
        // opening both would put a kernel on two planes for one session.
        let conversing = session.capabilities.conversation;
        self.renew_runtime_lease(lease.as_ref())
            .await
            .map_err(|error| {
                StartFailure::transient(format!(
                    "runtime authority expired before attachment: {error}"
                ))
            })?;
        let terminal = match conversing {
            false => Some(adapter.attach_terminal(&handle).await.map_err(|e| {
                StartFailure::transient(format!("could not attach to the runtime: {e}"))
            })?),
            true => None,
        };
        /*
         * Layer 2 owns the receiver until opening succeeds.
         *
         * An adapter can replay more than one channel's capacity before it
         * returns, so opening and receiving must be polled together. Ordinary
         * updates stay process-local until the candidate becomes authoritative;
         * sign-in is the explicit exception because the adapter may wait inside
         * `open_conversation` for its owner to act on that prompt.
         */
        let (conversation, mut conversation_opening) = match conversing {
            true => {
                self.renew_runtime_lease(lease.as_ref())
                    .await
                    .map_err(|error| {
                        StartFailure::transient(format!(
                            "runtime authority expired before opening its conversation: {error}"
                        ))
                    })?;
                let opening_authority = self
                    .acquire_conversation_opening(
                        &id,
                        &handle.0,
                        runtime_generation,
                        candidate.as_ref(),
                        lease.as_ref(),
                    )
                    .await
                    .map_err(|error| {
                        StartFailure::transient(format!(
                            "durable conversation opening authority could not be acquired: {error}"
                        ))
                    })?;
                let (updates_tx, mut opening) =
                    crate::conversation_opening::ConversationOpening::channel(
                        CONVERSATION_BUFFER,
                        opening_authority,
                        Arc::clone(&self.store),
                    );
                let renewal_authority = opening.authority().clone();
                let opening_result: Result<_, StartFailure> = {
                    let open = adapter.open_conversation(&handle, &options, updates_tx);
                    tokio::pin!(open);
                    let renewal = tokio::time::sleep(
                        crate::provision_support::CONVERSATION_OPENING_RENEW_INTERVAL,
                    );
                    tokio::pin!(renewal);
                    let mut receiving = true;
                    loop {
                        tokio::select! {
                            result = &mut open => break Ok(result),
                            () = &mut renewal => {
                                if let Err(error) = self
                                    .renew_conversation_opening(
                                        &renewal_authority,
                                        lease.as_ref(),
                                    )
                                    .await
                                {
                                    break Err(StartFailure::transient(format!(
                                        "durable conversation opening authority could not be renewed: {error}"
                                    )));
                                }
                                renewal.as_mut().reset(
                                    tokio::time::Instant::now()
                                        + crate::provision_support::CONVERSATION_OPENING_RENEW_INTERVAL,
                                );
                            }
                            update = opening.receive(), if receiving => match update {
                                Some(ConversationUpdate::SignInRequired { url, code, message }) => {
                                    if !opening.can_publish_sign_in() {
                                        break Err(StartFailure::permanent(
                                            "the runtime emitted an ordinary conversation update before its opening sign-in prompt".into(),
                                        ));
                                    }
                                    if let Err(error) = self
                                        .persist_opening_sign_in(
                                            &id,
                                            &renewal_authority,
                                            lease.as_ref(),
                                            url,
                                            code,
                                            message,
                                        )
                                        .await
                                    {
                                        break Err(StartFailure::transient(format!(
                                            "the opening sign-in prompt could not be recorded: {error}"
                                        )));
                                    }
                                }
                                Some(update) => opening.hold(update),
                                None => receiving = false,
                            }
                        }
                    }
                };
                match opening_result {
                    Ok(Ok(channel)) => (Some(channel), Some(opening)),
                    Ok(Err(error)) => {
                        self.revoke_conversation_opening(&id, opening).await;
                        // A kernel that advertised a conversation and cannot open one
                        // will not start being able to. Refused permanently so the
                        // owner is told now rather than after the backoff.
                        return Err(StartFailure::permanent(format!(
                            "could not open the conversation: {error}"
                        )));
                    }
                    Err(error) => {
                        self.revoke_conversation_opening(&id, opening).await;
                        return Err(error);
                    }
                }
            }
            false => (None, None),
        };

        let mut conversation_publication = None;
        let result = async {

        let named_conversation = conversation
            .as_ref()
            .and_then(|c| c.conversation_id.clone());

        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let (terminal_ending, _) = watch::channel(None);
        let (live_frames, _) = broadcast::channel(LIVE_BUFFER);
        let (resources, _) = broadcast::channel(RESOURCE_BUFFER);

        // Seeded from the record rather than from the copy passed in, because
        // the record is the authority and the copy is a snapshot from before
        // this task started. It has moved on since: progress events have been
        // appended against it, and a session being carried between runtimes
        // has had its version and rotation count written. Anything not read
        // back here is silently reverted by the next flush.
        let restore_context_ref = match self.store.get(&id).await {
            Ok(Some(record)) => record.context_ref,
            _ => None,
        };

        // Built for either plane, and fed only by the terminal. A conversation
        // session never writes to it — the honest shape would be one entry per
        // plane, which is a larger change than this and is worth making once
        // rather than halfway.
        let mut model = (self.screen_factory)(geometry);
        /*
         * Replayed into the new screen, exactly as readopt does. This path
         * serves a session being revived or given a replacement runtime, and
         * both are the same session: §5 asks that everything it has shown
         * remains visible, and "a session that returns blank has lost the work
         * as far as its owner can tell". The transcript is in storage either
         * way; not reading it here was the whole of the difference.
         */
        let restored = self
            .stored_transcript(restore_context_ref.as_deref())
            .await
            .unwrap_or_default();
        if !restored.is_empty() {
            model.apply(&restored);
            tracing::info!(session_id = %id, bytes = restored.len(), "transcript restored");
        }
        let screen: Arc<Mutex<Box<dyn ScreenModel>>> = Arc::new(Mutex::new(model));
        // What the capture sweep has already stored, so an unchanged screen is
        // not written back over itself on the next tick.
        let captured_len = screen.lock().await.repaint_bytes().len();

        let (terminal_input, terminal_output) = match terminal {
            Some(channel) => (Some(channel.input), Some(channel.output)),
            None => (None, None),
        };
        // Preparing a runtime takes long enough for its owner to stop, end, or
        // delete the session meanwhile. The store is the authority at the
        // installation boundary: publishing a live entry from the snapshot
        // passed to this task would otherwise revive a session after the
        // lifecycle operation had completed.
        let installation = self.lock_lifecycle(&id).await;
        let input_fence = self.input_gates.gate(&id).write_owned().await;
        let ordering = self.conversation_order_gates.gate(&id);
        let ordered_installation = ordering.lock().await;
        let observed = self.store.get(&id).await.map_err(|error| {
            StartFailure::transient(format!(
                "the session could not be checked before its runtime was installed: {error}"
            ))
        })?;
        let current = if lease.is_some() {
            let Some(record) = observed else {
                drop(installation);
                return Err(StartFailure::transient(
                    "the session disappeared before its candidate could become authoritative"
                        .into(),
                ));
            };
            let expected = lease
                .as_ref()
                .expect("the coordinated lease is present")
                .boundary
                .clone();
            let owns_rotation = handover
                .is_none_or(|handover| record.rotation_claim == handover.rotating.rotation_claim);
            if !record.session.wants_runtime()
                || record.session.kernel != session.kernel
                || record.session.status != expected.status
                || record.runtime_id != expected.authority.runtime_id
                || !owns_rotation
            {
                drop(installation);
                return Err(StartFailure::transient(
                    "the session changed before its candidate could become authoritative".into(),
                ));
            }
            self.renew_runtime_lease(lease.as_ref())
                .await
                .map_err(|error| {
                    StartFailure::transient(format!(
                        "runtime authority expired before the candidate switch: {error}"
                    ))
                })?;
            let candidate = candidate.as_ref().ok_or_else(|| {
                StartFailure::transient(
                    "a coordinated runtime reached installation without a candidate".into(),
                )
            })?;
            let writer_hold: Option<WorkspaceWriterHoldReceipt> = match handover {
                Some(handover) if handover.original.trigger_origin.is_some() => {
                    let expected_old = handover
                        .original
                        .runtime_id
                        .as_ref()
                        .map(|runtime| layer3_adapter::RuntimeHandle(runtime.clone()))
                        .ok_or_else(|| {
                            StartFailure::transient(
                                "the Trigger handover lost its old runtime before workspace capture"
                                    .into(),
                            )
                        })?;
                    let hold = handover.writer_hold.as_ref().ok_or_else(|| {
                        StartFailure::permanent(
                            "the Trigger handover reached H2 without its retained H1 writer hold"
                                .into(),
                        )
                    })?;
                    let expected_pair = handover.trigger_recovery.as_ref().ok_or_else(|| {
                        StartFailure::permanent(
                            "the Trigger handover reached H2 without its exact recovery descriptor"
                                .into(),
                        )
                    })?;
                    if hold.runtime != expected_old
                        || trigger_recovery.map(|pair| &pair.reference) != Some(expected_pair)
                    {
                        return Err(StartFailure::permanent(
                            "the Trigger handover candidate does not match its held recovery pair"
                                .into(),
                        ));
                    }
                    Some(hold.clone())
                }
                _ => None,
            };
            let first_switch = self
                .store
                .switch_runtime_authority(
                    lease.as_mut().expect("the coordinated lease is present"),
                    candidate,
                )
                .await;
            // An unavailable response can hide a committed switch. The same
            // candidate and lease token are an idempotent receipt; retry them
            // exactly before considering another generation.
            let switched_result = match first_switch {
                Ok(switched) => Ok(switched),
                Err(crate::store::StoreError::Unavailable(first_error)) => {
                    match self
                        .store
                        .switch_runtime_authority(
                            lease.as_mut().expect("the coordinated lease is present"),
                            candidate,
                        )
                        .await
                    {
                        Ok(switched) => Ok(switched),
                        Err(replay_error) => {
                            // The first response may hide a committed H2. No
                            // later error can prove that H1 is still the
                            // authority, so reopening its held writers here
                            // could create two concurrent repository writers.
                            // Keep the non-expiring hold and let readoption
                            // resolve the durable authority explicitly.
                            drop(installation);
                            return Err(StartFailure::unverified_attempt(format!(
                                "the runtime authority switch outcome is ambiguous after exact replay ({first_error}; {replay_error}); the old Trigger runtime remains writer-fenced"
                            )));
                        }
                    }
                }
                Err(error) => Err(error),
            };
            let switched = match switched_result {
                Ok(switched) => switched,
                Err(error) => {
                    if let Some(hold) = writer_hold.as_ref() {
                        adapter
                            .resume_workspace_writers(&hold.runtime, &hold.resume_token)
                            .await
                            .map_err(|resume_error| {
                                StartFailure::transient(format!(
                                    "the authority switch failed and the old Trigger runtime could not be reopened: {resume_error}"
                                ))
                            })?;
                    }
                    drop(installation);
                    return Err(StartFailure::transient(format!(
                        "the runtime candidate could not become authoritative: {error}"
                    )));
                }
            };
            if let Some(hold) = writer_hold.as_ref() {
                match self.store.runtime_cleanup_debts(&id).await {
                    Ok(debts) => {
                        if let Some(cleanup) = debts.into_iter().find(|cleanup| {
                            cleanup.runtime_id == hold.runtime.0
                                && cleanup.generation
                                    == handover
                                        .expect("a writer hold exists only for a handover")
                                        .rotating_boundary
                                        .authority
                                        .generation
                        }) {
                            if let Err(error) = self
                                .reclaim_runtime_with_lease(
                                    adapter,
                                    lease.as_ref().expect("the switched lease is present"),
                                    &cleanup,
                                )
                                .await
                            {
                                tracing::warn!(session_id = %id, runtime = %hold.runtime.0, %error, "held old Trigger runtime remains as exact cleanup debt");
                            }
                        } else {
                            tracing::warn!(session_id = %id, runtime = %hold.runtime.0, "authority switch did not expose the expected old-runtime cleanup debt");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(session_id = %id, runtime = %hold.runtime.0, %error, "cannot load the held old-runtime cleanup debt");
                    }
                }
            }
            // The switch writes its authority marker outside this process's
            // ordinary event lane. Advance the allocator before readiness or
            // handover completion can reserve the same sequence.
            self.event_logs
                .lane(&id)
                .observe_tail(switched.runtime_started_seq);
            let switched_status = lease
                .as_ref()
                .expect("the switched lease is present")
                .boundary
                .status;
            self.store
                .get(&id)
                .await
                .map_err(|error| {
                    StartFailure::transient(format!(
                        "the switched runtime authority could not be reloaded: {error}"
                    ))
                })?
                .filter(|record| {
                    record.runtime_id.as_deref() == Some(handle.0.as_str())
                        && record.session.status == switched_status
                        && handover.is_none_or(|handover| {
                            record.rotation_claim == handover.rotating.rotation_claim
                        })
                })
                .ok_or_else(|| {
                    StartFailure::transient(
                        "the switched runtime no longer owns its installation boundary".into(),
                    )
                })?
        } else {
            match observed {
                Some(record)
                    if record.session.wants_runtime()
                        && record.session.kernel == session.kernel
                        && record.runtime_id.as_deref() == Some(handle.0.as_str()) =>
                {
                    record
                }
                current => {
                    // The lifecycle gate keeps local publication, stop, and
                    // readoption behind this cleanup. Input and conversation
                    // ordering are already fenced by it and must not be held
                    // across a slow provider call.
                    drop(ordered_installation);
                    drop(input_fence);
                    let teardown = match current {
                        Some(record)
                            if record.runtime_id.as_deref() == Some(handle.0.as_str()) =>
                        {
                            self.teardown_compatibility_runtime(
                                adapter,
                                &record,
                                "superseded runtime cleanup",
                            )
                            .await
                            .map(|_| ())
                            .map_err(|error| {
                                StartFailure::transient(format!(
                                    "the superseded runtime could not be settled safely: {error}"
                                ))
                            })
                        }
                        // This candidate is still owned by the in-flight create
                        // that produced `handle`; it was never published as the
                        // current row's authority. A delete or successor row
                        // must therefore not prevent its stack owner from
                        // reclaiming it.
                        _ => adapter.terminate(&handle).await.map_err(|error| {
                            StartFailure::unverified_attempt(format!(
                                "the superseded runtime {} could not be torn down: {error}",
                                handle.0
                            ))
                        }),
                    };
                    drop(installation);
                    teardown?;
                    return Ok(());
                }
            }
        };
        let ready_claimed = current.clone();
        let mut ready = current;
        ready.session.geometry = geometry;
        ready.session.status = SessionStatus::Ready;
        // Whatever brought the session here, it is running now. Without this,
        // `recovering` would be a state a replacement session never leaves.
        ready.session.recovery_state = RecoveryState::Normal;
        ready.session.stop_reason = None;
        // The grant becomes public in the same commit that makes this exact
        // runtime interactive. Legacy fallback may legitimately have produced
        // a different answer; before Ready the session remains noninteractive.
        ready.session.granted_skills = Some(Self::granted_skills_for(skills));
        ready.skill_packs = if let Some(digest) = prepared.stored_digest {
            crate::store::SkillPackState::Stored { digest }
        } else {
            crate::store::SkillPackState::Staged(skills.clone())
        };
        ready.session.runtime_version = match ready.pending_handover.as_ref().map(|pending| {
            &pending.target
        }) {
            Some(crate::store::PendingHandoverTarget::RuntimeVersion {
                runtime_version,
                ..
            }) => {
                Some(runtime_version.clone())
            }
            _ => self.runtime_version.clone(),
        };
        let mut ready_events = handover
            .map(|handover| handover.completion_events.clone())
            .unwrap_or_default();
        if handover.is_some() {
            ready.session.options = session.options.clone();
            ready.session.capabilities = session.capabilities;
            ready.session.rotations = session.rotations;
            ready.session.rotated_at.clone_from(&session.rotated_at);
        }
        ready_events.push(Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
                detail: None,
                capability: None,
            },
            appended_at: String::new(),
        });
        if let Some(conversation_id) = named_conversation {
            ready_events.push(Event {
                session_id: id.clone(),
                seq: 0,
                kind: EventKind::ConversationOpened { conversation_id },
                appended_at: String::new(),
            });
        }
        let readiness = match handover {
            Some(handover) => {
                self.complete_handover_durably(&mut ready, &handover.rotating, ready_events)
                    .await
            }
            None => {
                self.publish_ready_durably(&mut ready, &ready_claimed, ready_events)
                    .await
            }
        };
        readiness.map_err(|error| {
            StartFailure::transient(format!(
                "the runtime became usable but readiness could not be recorded: {error}"
            ))
        })?;
        if let Some(held) = lease.as_mut() {
            let boundary = SessionBoundary::for_record(
                &ready,
                candidate
                    .as_ref()
                    .expect("a coordinated runtime has a candidate")
                    .generation
                    .get(),
            );
            self.advance_runtime_lease(held, &boundary)
                .await
                .map_err(|error| {
                    StartFailure::transient(format!(
                        "readiness committed but its runtime lease could not advance: {error}"
                    ))
                })?;
        }
        if let (Some(workspaces), Some(origin)) =
            (self.workspaces.as_ref(), ready.trigger_origin.as_ref())
        {
            workspaces
                .synchronize_runtime_generation(
                    &ready.session.owner,
                    origin,
                    runtime_generation,
                    &crate::clock::now_rfc3339(),
                )
                .await
                .map_err(|error| {
                    StartFailure::transient(format!(
                        "readiness committed but the Trigger Launch runtime authority could not advance: {error}"
                    ))
                })?;
        }
        #[cfg(test)]
        self.pause_live_install_if_armed().await;

        // Reconstruct from the final ready record. Provisioning can revive a
        // session that already had an unanswered request, and the opening FIFO
        // may add another one immediately after this snapshot.
        let final_history = self.readoption_history(&ready).await.map_err(|error| {
            StartFailure::transient(format!(
                "the runtime became usable but its conversation state could not be rebuilt: {error}"
            ))
        })?;

        // Publication is the durable cross-replica handover. It happens only
        // after readiness and final history are known, but before any buffered
        // frame can be folded or the process-local entry becomes visible.
        let (conversation_link, conversation_authority, opening_updates, conversation_updates) =
            match conversation_opening.take() {
                Some(opening) => {
                    if let Err(error) = self
                        .renew_conversation_opening(opening.authority(), lease.as_ref())
                        .await
                    {
                        opening.revoke().await;
                        return Err(StartFailure::transient(format!(
                            "conversation opening authority expired before publication: {error}"
                        )));
                    }
                    let (link, durable, held, updates, publication) =
                        opening.publish(lease.as_ref()).await.map_err(|error| {
                            StartFailure::transient(format!(
                                "the conversation link could not be published durably: {error}"
                            ))
                        })?;
                    conversation_publication = Some(publication);
                    (Some(link), Some(durable), Some(held), Some(updates))
                }
                None => (None, None, None, None),
            };

        let durable = ready.clone();
        let seeded = ready.session;
        let context_ref = ready.context_ref;
        let event_log = self.event_logs.lane(&id);
        event_log.observe_tail(seeded.last_seq);
        event_log.observe_record(&durable);
        let entry = SessionEntry {
            session: seeded,
            trigger_origin: durable.trigger_origin.clone(),
            initial_prompt: durable.initial_prompt.clone(),
            event_log,
            // Evidence means the adapter acknowledged this exact value, not
            // merely that provisioning intends to send it.
            subject_credential: None,
            environment_values,
            screen: Arc::clone(&screen),
            runtime: Some(handle.clone()),
            runtime_generation,
            publication_token: publication_token.to_string(),
            publication_complete: false,
            conversation_link,
            conversation_authority,
            input: terminal_input.clone(),
            live: Some(live.clone()),
            terminal_ending: terminal_ending.clone(),
            live_frames: live_frames.clone(),
            resources,
            last_reading: None,
            memory_pressure: Default::default(),
            readings_unreported: false,
            conversation: conversation.as_ref().map(|c| c.input.clone()),
            pending_approvals: final_history.pending_approvals,
            approval_locators: final_history.approval_locators,
            assembling: None,
            context_reading: None,
            attached: None,
            attachments_issued: 0,
            context_ref,
            skill_packs: durable.skill_packs,
            captured_len,
        };
        let replaced = self.entries.write().await.insert(id.clone(), entry);
        if let (Some(handover), Some(replaced)) = (handover, replaced) {
            if let Some(attached) = replaced.attached.as_ref() {
                let _ = attached.supersede.send(handover.detached);
            }
        }
        if let (Some(link), Some(updates)) = (conversation_link, opening_updates) {
            if !self.fold_opening_updates_locked(&id, link, updates).await {
                return Err(StartFailure::transient(
                    "the conversation link was superseded during opening replay".into(),
                ));
            }
        }
        if let Some(held) = lease.as_mut() {
            let replayed = self
                .store
                .get(&id)
                .await
                .map_err(|error| {
                    StartFailure::transient(format!(
                        "opening replay committed but its runtime boundary could not be reloaded: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    StartFailure::transient(
                        "the session disappeared while its opening replay was published".into(),
                    )
                })?;
            let boundary = SessionBoundary::for_record(&replayed, runtime_generation);
            if boundary != held.boundary {
                self.advance_runtime_lease(held, &boundary)
                    .await
                    .map_err(|error| {
                        StartFailure::transient(format!(
                            "opening replay committed but its runtime lease could not advance: {error}"
                        ))
                    })?;
            }
        }

        // The durable provider-operation lease fences this exact runtime from
        // another replica until acknowledgement and evidence recording finish.
        // The local installation, input, and conversation-order gates keep this
        // process from crossing the same unacknowledged credential boundary.
        // Refusal is not fatal: the runtime remains usable and the top-up sweep
        // retries while the marker stays empty.
        if self.credentials.is_some() {
            match self
                    .install_prepared_credential_while_lifecycle_locked(
                        &installation,
                        session,
                        spec.caller_credential.as_ref(),
                        &handle,
                    runtime_generation,
                    lease.as_ref(),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        session_id = %id,
                        "the runtime started without its owner's credential"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        session_id = %id,
                        %error,
                        "the runtime's credential could not be recorded"
                    );
                }
            }
        }
        if let Some(input) = &terminal_input {
            let _ = input
                .send(TerminalInput::Resize(TermSize {
                    cols: geometry.columns(),
                    rows: geometry.rows(),
                }))
                .await;
        }

        self.note_progress(&id, ProvisioningStage::Ready).await;
        {
            // This lock acquisition is the final cancellation point before
            // publication. Every consumer is spawned and the entry is marked
            // complete synchronously under one exact identity guard, so cleanup
            // can never remove the entry while orphan tasks keep its channels.
            let mut entries = self.entries.write().await;
            let exact = entries.get(&id).is_some_and(|entry| {
                entry.runtime.as_ref() == Some(&handle)
                    && entry.runtime_generation == runtime_generation
                    && entry.publication_token == publication_token
                    && Arc::ptr_eq(&entry.screen, &screen)
            });
            if !exact {
                return Err(StartFailure::transient(
                    "the runtime entry changed before publication completed".into(),
                ));
            }
            if let (Some(link), Some(updates)) = (conversation_link, conversation_updates) {
                crate::conversation_log::spawn_folder(
                    Arc::clone(self),
                    id.clone(),
                    link,
                    runtime_generation,
                    updates,
                );
                conversation_publication
                    .take()
                    .expect("a published conversation receiver retains its cleanup guard")
                    .disarm();
            }
            crate::resources::spawn_resource_watch(
                Arc::clone(self),
                id.clone(),
                Arc::clone(adapter),
                handle.clone(),
            );
            // Both consumers run for the session's life, independent of any
            // client. A session nobody is watching still paints its screen and
            // records what its agent did.
            if let Some(output) = terminal_output {
                spawn_relay(
                    Arc::clone(self),
                    id.clone(),
                    output,
                    Arc::clone(&screen),
                    live,
                    terminal_ending,
                );
            }
            entries
                .get_mut(&id)
                .expect("the exact entry remains locked")
                .publication_complete = true;
        }
        drop(ordered_installation);
        drop(input_fence);
        drop(installation);
        tracing::info!(session_id = %id, "runtime ready");
        Ok(())
        }
        .await;
        if let Some(publication) = conversation_publication.take() {
            self.revoke_conversation_publication(&id, publication).await;
        }
        if let Some(opening) = conversation_opening.take() {
            self.revoke_conversation_opening(&id, opening).await;
        }
        result
    }
}

impl SessionService {
    /// Resolve and integrity-check the repository package for this generation.
    pub(crate) async fn workspace_for(
        &self,
        id: &crate::SessionId,
    ) -> Result<Option<layer3_adapter::WorkspacePackage>, StartFailure> {
        let record = self.store.get(id).await.map_err(|error| {
            StartFailure::transient(format!(
                "this session's workspace authority could not be read: {error}"
            ))
        })?;
        let Some(record) = record else {
            return Err(StartFailure::permanent(
                "this Trigger session no longer exists".to_string(),
            ));
        };
        let Some(origin) = record.trigger_origin else {
            return Ok(None);
        };
        let source = self.workspaces.as_ref().ok_or_else(|| {
            StartFailure::permanent(
                "this Trigger session has no configured workspace source".to_string(),
            )
        })?;
        let workspace = source
            .resolve_workspace(&record.session.owner, &origin)
            .await
            .map_err(|error| match error {
                crate::store::StoreError::Unavailable(detail) => StartFailure::transient(format!(
                    "this Trigger session's workspace could not be loaded: {detail}"
                )),
                other => StartFailure::permanent(format!(
                    "this Trigger session's workspace authority is malformed: {other}"
                )),
            })?
            .ok_or_else(|| {
                StartFailure::permanent(
                    "this Trigger session's repository workspace is missing".to_string(),
                )
            })?;
        let digest = format!("{:x}", Sha256::digest(&workspace.archive));
        if workspace.epoch != origin.workspace_epoch
            || workspace.source_revision != origin.source_revision
            || workspace.baseline_manifest_digest != origin.manifest_digest
            || workspace.manifest_digest.len() != 64
            || !workspace
                .manifest_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || workspace.archive_sha256.len() != 64
            || !workspace
                .archive_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || digest != workspace.archive_sha256
        {
            return Err(StartFailure::permanent(
                "this Trigger session's repository workspace failed integrity verification"
                    .to_string(),
            ));
        }
        Ok(Some(workspace))
    }
}
