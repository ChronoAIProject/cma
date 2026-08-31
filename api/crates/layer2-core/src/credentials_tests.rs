//! Tests for delegated-credential renewal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::KernelAdapter;
use tokio::sync::Notify;

use crate::authz::{Authorization, Caller, Capability};
use crate::coordination::{SessionBoundary, SessionLease, SessionLeaseRequest, SessionOperation};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::SessionService;
use crate::session::SessionSpec;
use crate::store::{CredentialSource, SessionStore, StoreError};
use crate::terminal::ScreenModel;
use crate::test_support::{await_settled, FakeAdapter, FakeStore, NullScreen};
use crate::CredentialDeliveryTarget;

/// A runtime that records the credentials it was handed, so the service's
/// behaviour is observable without a real sandbox.
///
/// It records them at `refresh_credential` rather than by watching terminal
/// input, which is the point of these tests now: a credential reaches a
/// runtime because the adapter was asked for it, not because it happened to
/// share a channel with keystrokes.
struct RecordingAdapter {
    sent: Arc<tokio::sync::Mutex<Vec<String>>>,
    refuse: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl KernelAdapter for RecordingAdapter {
    fn kernel_name(&self) -> &'static str {
        "recording"
    }

    fn capabilities(&self) -> layer3_adapter::KernelCapabilities {
        layer3_adapter::KernelCapabilities {
            terminal: true,
            snapshot: true,
            ..Default::default()
        }
    }

    async fn provision(
        &self,
        spec: &layer3_adapter::RuntimeSpec,
    ) -> Result<layer3_adapter::RuntimeHandle, layer3_adapter::AdapterError> {
        Ok(layer3_adapter::RuntimeHandle(spec.session_id.clone()))
    }

    async fn attach_terminal(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
    ) -> Result<layer3_adapter::TerminalChannel, layer3_adapter::AdapterError> {
        let (_out_tx, out_rx) = tokio::sync::mpsc::channel::<layer3_adapter::TerminalOutput>(16);
        Ok(layer3_adapter::TerminalChannel {
            output: out_rx,
            input: layer3_adapter::TerminalInputSender::spawn(16, |_input| async { Ok(()) }),
        })
    }

    async fn refresh_credential(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
        credential: &layer3_adapter::RuntimeCredential,
    ) -> Result<(), layer3_adapter::AdapterError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Unavailable(
                "the runtime did not take the credential".into(),
            ));
        }
        self.sent.lock().await.push(credential.expose().to_string());
        Ok(())
    }

    async fn terminate(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
    ) -> Result<(), layer3_adapter::AdapterError> {
        Ok(())
    }
}

/// An adapter whose next acknowledgement can be held while another lifecycle
/// operation contends for the same session gate.
#[derive(Default)]
struct BlockingAdapter {
    sent: tokio::sync::Mutex<Vec<(String, String)>>,
    block_next: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl BlockingAdapter {
    fn arm(&self) {
        assert!(!self.block_next.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified())
            .await
            .expect("credential delivery reached the adapter");
    }

    fn acknowledge(&self) {
        self.release.notify_one();
    }
}

#[async_trait::async_trait]
impl KernelAdapter for BlockingAdapter {
    fn kernel_name(&self) -> &'static str {
        "recording"
    }

    fn capabilities(&self) -> layer3_adapter::KernelCapabilities {
        layer3_adapter::KernelCapabilities {
            terminal: true,
            snapshot: true,
            ..Default::default()
        }
    }

    async fn provision(
        &self,
        spec: &layer3_adapter::RuntimeSpec,
    ) -> Result<layer3_adapter::RuntimeHandle, layer3_adapter::AdapterError> {
        Ok(layer3_adapter::RuntimeHandle(spec.session_id.clone()))
    }

    async fn attach_terminal(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
    ) -> Result<layer3_adapter::TerminalChannel, layer3_adapter::AdapterError> {
        let (_out_tx, out_rx) = tokio::sync::mpsc::channel::<layer3_adapter::TerminalOutput>(16);
        Ok(layer3_adapter::TerminalChannel {
            output: out_rx,
            input: layer3_adapter::TerminalInputSender::spawn(16, |_input| async { Ok(()) }),
        })
    }

    async fn refresh_credential(
        &self,
        handle: &layer3_adapter::RuntimeHandle,
        credential: &layer3_adapter::RuntimeCredential,
    ) -> Result<(), layer3_adapter::AdapterError> {
        if self.block_next.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.sent
            .lock()
            .await
            .push((handle.0.clone(), credential.expose().to_string()));
        Ok(())
    }

    async fn terminate(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
    ) -> Result<(), layer3_adapter::AdapterError> {
        Ok(())
    }
}

/// Durable credential selection shared by two service replicas in race tests.
struct SharedCredentialSource {
    value: tokio::sync::Mutex<String>,
    pause_next_read: AtomicBool,
    read_entered: Notify,
    read_release: Notify,
}

impl SharedCredentialSource {
    fn new(value: &str) -> Arc<Self> {
        Arc::new(Self {
            value: tokio::sync::Mutex::new(value.to_string()),
            pause_next_read: AtomicBool::new(false),
            read_entered: Notify::new(),
            read_release: Notify::new(),
        })
    }

    async fn replace(&self, value: &str) {
        *self.value.lock().await = value.to_string();
    }

    async fn current(&self) -> String {
        self.value.lock().await.clone()
    }

    fn pause_next_read(&self) {
        assert!(!self.pause_next_read.swap(true, Ordering::SeqCst));
    }

    async fn wait_until_read_paused(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.read_entered.notified())
            .await
            .expect("credential selection reaches its pause");
    }

    fn resume_read(&self) {
        self.read_release.notify_one();
    }
}

#[async_trait::async_trait]
impl CredentialSource for SharedCredentialSource {
    async fn credential_for(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
        _caller_credential: Option<&crate::session::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        let selected = self.current().await;
        if self.pause_next_read.swap(false, Ordering::SeqCst) {
            self.read_entered.notify_one();
            self.read_release.notified().await;
        }
        Ok(Some(selected))
    }

    async fn release(&self, _session: &SessionId) -> Result<(), StoreError> {
        Ok(())
    }

    async fn tracks_subject_credential(&self, _session: &SessionId) -> Result<bool, StoreError> {
        Ok(true)
    }
}

/// Current/pending custody with exact runtime-target binding.
///
/// This is the smallest in-memory model of CMA's generation-CAS contract. It
/// lets a test pause H1 only after the adapter has acknowledged B, then have H2
/// atomically retarget and promote that same pending value while H1's finalizer
/// is still outstanding.
struct CorrelatedCredentialSource {
    state: tokio::sync::Mutex<CorrelatedCredentialState>,
    block_next_confirmation: AtomicBool,
    confirmation_entered: Notify,
    confirmation_release: Notify,
}

struct CorrelatedCredentialState {
    current: String,
    pending: Option<String>,
    pending_target: Option<CredentialDeliveryTarget>,
    revoke_debt: Vec<String>,
    preparations: Vec<(CredentialDeliveryTarget, String)>,
    confirmations: Vec<(CredentialDeliveryTarget, String, bool)>,
}

impl CorrelatedCredentialSource {
    fn new(current: &str) -> Arc<Self> {
        Arc::new(Self {
            state: tokio::sync::Mutex::new(CorrelatedCredentialState {
                current: current.to_string(),
                pending: None,
                pending_target: None,
                revoke_debt: Vec::new(),
                preparations: Vec::new(),
                confirmations: Vec::new(),
            }),
            block_next_confirmation: AtomicBool::new(false),
            confirmation_entered: Notify::new(),
            confirmation_release: Notify::new(),
        })
    }

    async fn seed_pending(&self, value: &str) {
        let mut state = self.state.lock().await;
        assert!(state.pending.is_none(), "pending custody is already seeded");
        state.pending = Some(value.to_string());
        state.pending_target = None;
    }

    fn block_next_confirmation(&self) {
        assert!(
            !self.block_next_confirmation.swap(true, Ordering::SeqCst),
            "a credential confirmation pause is already armed"
        );
    }

    async fn wait_until_confirmation_paused(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.confirmation_entered.notified())
            .await
            .expect("post-acknowledgement credential confirmation reached its pause");
    }

    fn resume_confirmation(&self) {
        self.confirmation_release.notify_one();
    }
}

#[async_trait::async_trait]
impl CredentialSource for CorrelatedCredentialSource {
    async fn credential_for(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
        _caller_credential: Option<&crate::session::CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        Ok(Some(self.state.lock().await.current.clone()))
    }

    async fn prepare_credential_delivery(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
        _caller_credential: Option<&crate::session::CallerCredential>,
        target: &CredentialDeliveryTarget,
    ) -> Result<Option<layer3_adapter::RuntimeCredential>, StoreError> {
        let mut state = self.state.lock().await;
        let selected = match state.pending.clone() {
            Some(pending) => {
                state.pending_target = Some(target.clone());
                pending
            }
            None => state.current.clone(),
        };
        state.preparations.push((target.clone(), selected.clone()));
        Ok(Some(layer3_adapter::RuntimeCredential::new(selected)))
    }

    async fn confirm_credential_delivery(
        &self,
        _session: &SessionId,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
        delivered_value: &str,
        target: &CredentialDeliveryTarget,
    ) -> Result<bool, StoreError> {
        if self.block_next_confirmation.swap(false, Ordering::SeqCst) {
            self.confirmation_entered.notify_one();
            self.confirmation_release.notified().await;
        }

        let mut state = self.state.lock().await;
        let confirmed = if state.current == delivered_value {
            true
        } else if state.pending.as_deref() == Some(delivered_value)
            && state.pending_target.as_ref() == Some(target)
        {
            let replacement = state
                .pending
                .take()
                .expect("the exact pending value was checked above");
            state.pending_target = None;
            let displaced = std::mem::replace(&mut state.current, replacement);
            state.revoke_debt.push(displaced);
            true
        } else {
            false
        };
        state
            .confirmations
            .push((target.clone(), delivered_value.to_string(), confirmed));
        Ok(confirmed)
    }

    async fn release(&self, _session: &SessionId) -> Result<(), StoreError> {
        Ok(())
    }

    async fn tracks_subject_credential(&self, _session: &SessionId) -> Result<bool, StoreError> {
        Ok(true)
    }
}

/// A caller holding the everyday capabilities, so these tests exercise the
/// credential path rather than authorization.
fn caller(id: &str) -> Caller {
    Caller::new(
        SubjectId(id.to_string()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
        ]),
    )
}

/// A source that mints a value visibly distinct from anything a caller holds.
///
/// The whole point of the mint is that what the runtime presents is not what
/// the person presented, so a fake that echoed the caller's token back would
/// pass whether or not the mint happened.
struct MintingSource {
    observed_callers: Option<Arc<tokio::sync::Mutex<Vec<Option<String>>>>>,
    tracks_subject: bool,
}

#[async_trait::async_trait]
impl crate::store::CredentialSource for MintingSource {
    async fn credential_for(
        &self,
        session: &crate::SessionId,
        _subject: &crate::SubjectId,
        _services: &[crate::environment::ServiceId],
        caller_credential: Option<&crate::session::CallerCredential>,
    ) -> Result<Option<String>, crate::store::StoreError> {
        if let Some(observed) = &self.observed_callers {
            observed
                .lock()
                .await
                .push(caller_credential.map(|credential| credential.as_ref().to_owned()));
        }
        Ok(Some(format!("minted-for-{}", session.0)))
    }

    /// Nothing to reclaim: this mints a value rather than registering one.
    async fn release(&self, _session: &crate::SessionId) -> Result<(), crate::store::StoreError> {
        Ok(())
    }

    async fn tracks_subject_credential(
        &self,
        _session: &crate::SessionId,
    ) -> Result<bool, crate::store::StoreError> {
        Ok(self.tracks_subject)
    }
}

fn service(sent: Arc<tokio::sync::Mutex<Vec<String>>>) -> Arc<SessionService> {
    service_with_store(sent, Arc::new(FakeStore::default()), None, true)
}

fn service_with_store(
    sent: Arc<tokio::sync::Mutex<Vec<String>>>,
    store: Arc<FakeStore>,
    observed_callers: Option<Arc<tokio::sync::Mutex<Vec<Option<String>>>>>,
    tracks_subject: bool,
) -> Arc<SessionService> {
    service_with_store_and_refusal(
        sent,
        store,
        observed_callers,
        tracks_subject,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}

fn service_with_store_and_refusal(
    sent: Arc<tokio::sync::Mutex<Vec<String>>>,
    store: Arc<FakeStore>,
    observed_callers: Option<Arc<tokio::sync::Mutex<Vec<Option<String>>>>>,
    tracks_subject: bool,
    refuse: Arc<AtomicBool>,
) -> Arc<SessionService> {
    service_with_adapter(
        Arc::new(RecordingAdapter { sent, refuse }),
        store,
        observed_callers,
        tracks_subject,
    )
}

fn service_with_adapter(
    adapter: Arc<dyn KernelAdapter>,
    store: Arc<FakeStore>,
    observed_callers: Option<Arc<tokio::sync::Mutex<Vec<Option<String>>>>>,
    tracks_subject: bool,
) -> Arc<SessionService> {
    service_with_adapter_and_credentials(
        adapter,
        store,
        Arc::new(MintingSource {
            observed_callers,
            tracks_subject,
        }),
    )
}

fn service_with_adapter_and_credentials(
    adapter: Arc<dyn KernelAdapter>,
    store: Arc<FakeStore>,
    credentials: Arc<dyn CredentialSource>,
) -> Arc<SessionService> {
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("recording"), adapter);
    // A real store rather than a stub: these tests care that a credential
    // reaches the runtime, and that path now runs through persistence.
    SessionService::new(
        adapters,
        Arc::new(|g| Box::new(NullScreen(g)) as Box<dyn ScreenModel>),
        store,
        crate::service::SessionPorts {
            credentials: Some(credentials),
            ..Default::default()
        },
        crate::environment_service::EnvironmentProfileService::new(
            Arc::new(crate::test_support::FakeProfileStore::default()),
            crate::environment::Baseline::default(),
            None,
            None,
            crate::environment::AgentConfiguration {
                kernel: KernelId::new("recording"),
                ..Default::default()
            },
            crate::test_support::unconfigured_settings(),
        ),
        crate::test_support::owned(),
    )
}

async fn acquire_handover(
    service: &Arc<SessionService>,
    store: &Arc<FakeStore>,
    id: &SessionId,
) -> Result<SessionLease, StoreError> {
    let record = store.get(id).await?.expect("the session remains durable");
    let authority = store
        .runtime_authority(id)
        .await?
        .expect("the session has runtime authority");
    store
        .acquire_session_lease(&SessionLeaseRequest::new(
            id.clone(),
            service.coordination_holder.clone(),
            SessionBoundary::for_record(&record, authority.generation),
            SessionOperation::Handover,
            Duration::from_secs(30),
        ))
        .await
}

async fn switch_handover_to_h2(
    store: &Arc<FakeStore>,
    lease: &mut SessionLease,
) -> crate::coordination::RuntimeAuthoritySwitch {
    let reservation = store
        .reserve_runtime_generation(lease, "credential-race-h2")
        .await
        .expect("H2 generation is reserved under handover");
    let candidate = store
        .register_runtime_candidate(lease, &reservation, "runtime-h2")
        .await
        .expect("H2 is registered under handover");
    store
        .switch_runtime_authority(lease, &candidate)
        .await
        .expect("H2 becomes the durable authority")
}

async fn wait_until_session_lease_released(store: &Arc<FakeStore>, id: &SessionId) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.active_session_lease(id).await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the service-owned interaction releases its durable lease");
}

fn service_with_delivery_switch(
    sent: Arc<tokio::sync::Mutex<Vec<String>>>,
) -> (Arc<SessionService>, Arc<AtomicBool>) {
    let refuse = Arc::new(AtomicBool::new(false));
    let service = service_with_store_and_refusal(
        sent,
        Arc::new(FakeStore::default()),
        None,
        true,
        Arc::clone(&refuse),
    );
    (service, refuse)
}

async fn credentials_seen(sent: &Arc<tokio::sync::Mutex<Vec<String>>>) -> Vec<String> {
    sent.lock().await.clone()
}

#[tokio::test]
async fn the_runtime_is_given_a_minted_credential_and_never_its_owner_s_own() {
    /*
     * The property the mint exists for, and the one that was not holding.
     *
     * An interface put the caller's own access token on the spec so the
     * platform could be asked questions as them. Provisioning read that field,
     * found it filled, and skipped minting — so every session a person created
     * ran on their whole identity, while the minting machinery ran only for
     * sessions the controller recovered. The list below is what the runtime
     * was actually handed, and "first-token" appearing in it is the defect.
     */
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let observed_callers = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service_with_store(
        Arc::clone(&sent),
        Arc::new(FakeStore::default()),
        Some(Arc::clone(&observed_callers)),
        true,
    );
    let owner = caller("subject-a");
    let session = svc
        .create_session(
            &owner,
            SessionSpec {
                surface: None,
                environment_profile: None,
                geometry: None,
                options: Default::default(),
                caller_credential: Some("first-token".into()),
                runtime_credential: None,
                requested_mode: None,
                quoted_rate_card: None,
            },
        )
        .await
        .expect("session created");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let seen = credentials_seen(&sent).await;
    assert_eq!(
        seen,
        vec![format!("minted-for-{}", session.id.0)],
        "the runtime must present what was minted for this session"
    );
    assert!(
        !seen.iter().any(|c| c == "first-token"),
        "the caller's own credential must never reach the runtime, got {seen:?}"
    );
    let observed = observed_callers.lock().await;
    assert_eq!(
        observed.len(),
        2,
        "custody is established before provisioning and selected again for the exact runtime"
    );
    assert!(
        observed
            .iter()
            .all(|caller| caller.as_deref() == Some("first-token")),
        "both source boundaries need the initiating request's authority, got {observed:?}"
    );
}

#[tokio::test]
async fn a_renewal_reaches_a_running_session_without_recreating_it() {
    // The property the design exists for: the session keeps running and simply
    // starts presenting the new value.
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service(Arc::clone(&sent));
    let owner = caller("subject-a");
    let session = svc
        .create_session(
            &owner,
            SessionSpec {
                surface: None,
                environment_profile: None,
                geometry: None,
                options: Default::default(),
                caller_credential: Some("first-token".into()),
                runtime_credential: None,
                requested_mode: None,
                quoted_rate_card: None,
            },
        )
        .await
        .expect("session created");

    // A renewal reaches sessions that are running, and the runtime only starts
    // after creation returns.
    await_settled(&svc, &owner, &session.id).await;
    let pushed = svc
        .refresh_subject_credential(&owner.subject, "second-token".into())
        .await;
    assert_eq!(pushed, 1, "the live session must receive the renewal");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let seen = credentials_seen(&sent).await;
    assert_eq!(
        seen,
        vec![
            format!("minted-for-{}", session.id.0),
            "second-token".to_string()
        ],
        "the session starts on what was minted for it, then takes the renewal"
    );

    // Still the same session: renewal must not replace it.
    let after = svc
        .get_session(&owner, &session.id)
        .await
        .expect("session still exists");
    assert_eq!(after.id, session.id);
}

#[tokio::test]
async fn a_subject_renewal_does_not_replace_a_request_bound_session_credential() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service_with_store(
        Arc::clone(&sent),
        Arc::new(FakeStore::default()),
        None,
        false,
    );
    let owner = caller("subject-a");
    let session = svc
        .create_session(
            &owner,
            SessionSpec {
                surface: None,
                environment_profile: None,
                geometry: None,
                options: Default::default(),
                caller_credential: Some(crate::CallerCredential::request_bound(
                    "bearer-token".into(),
                )),
                runtime_credential: None,
                requested_mode: None,
                quoted_rate_card: None,
            },
        )
        .await
        .expect("session created");
    await_settled(&svc, &owner, &session.id).await;

    let pushed = svc
        .refresh_subject_credential(&owner.subject, "browser-renewal".into())
        .await;
    assert_eq!(pushed, 0);
    assert!(
        !credentials_seen(&sent)
            .await
            .contains(&"browser-renewal".to_string()),
        "a browser renewal must not overwrite request-bound authority"
    );
}

#[tokio::test]
async fn a_renewal_does_not_leak_across_subjects() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service(Arc::clone(&sent));
    let owner = caller("subject-a");
    svc.create_session(
        &owner,
        SessionSpec {
            surface: None,
            environment_profile: None,
            geometry: None,
            options: Default::default(),
            caller_credential: Some("a-token".into()),
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
        },
    )
    .await
    .expect("session created");

    let pushed = svc
        .refresh_subject_credential(&SubjectId("subject-b".to_string()), "b-token".into())
        .await;
    assert_eq!(pushed, 0, "another subject's renewal must reach nothing");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!credentials_seen(&sent)
        .await
        .contains(&"b-token".to_string()));
}

#[tokio::test]
async fn a_terminated_session_is_not_renewed() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service(Arc::clone(&sent));
    let owner = caller("subject-a");
    let session = svc
        .create_session(
            &owner,
            SessionSpec {
                surface: None,
                environment_profile: None,
                geometry: None,
                options: Default::default(),
                caller_credential: Some("token".into()),
                runtime_credential: None,
                requested_mode: None,
                quoted_rate_card: None,
            },
        )
        .await
        .expect("session created");
    svc.terminate_session(&owner, &session.id)
        .await
        .expect("terminated");

    let pushed = svc
        .refresh_subject_credential(&owner.subject, "new-token".into())
        .await;
    assert_eq!(pushed, 0, "a dead session must not be handed a credential");
}

#[tokio::test]
async fn a_session_is_minted_a_credential_even_when_the_caller_delegated_none() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service(Arc::clone(&sent));
    let owner = caller("subject-a");
    svc.create_session(
        &owner,
        SessionSpec {
            surface: None,
            environment_profile: None,
            geometry: None,
            caller_credential: None,
            runtime_credential: None,
            requested_mode: None,
            quoted_rate_card: None,
            options: Default::default(),
        },
    )
    .await
    .expect("a session with no delegation still starts");

    /*
     * What the runtime may spend is decided by what the session was granted,
     * not by whether the caller happened to hand something over. This used to
     * be the other way round — no delegation meant no credential — which is
     * the same coupling that let a delegation become the session's credential
     * and skip the mint entirely.
     */
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let seen = credentials_seen(&sent).await;
    assert_eq!(
        seen.len(),
        1,
        "a credential is minted regardless, got {seen:?}"
    );
    assert!(
        seen[0].starts_with("minted-for-"),
        "and it is a minted one, got {seen:?}"
    );
}

#[tokio::test]
async fn a_refused_credential_is_neither_recorded_nor_evidence_for_reclaim() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (svc, refuse) = service_with_delivery_switch(sent);
    let owner = caller("delivery-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;

    let working = svc
        .credential_held(&session.id)
        .await
        .expect("the running session has its initial credential");
    assert!(svc
        .can_reclaim_untracked_credential(&session.id, &[working.as_str()])
        .await
        .expect("session store works"));

    refuse.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        !svc.push_session_credential(&session.id, "undelivered-replacement".into())
            .await
    );
    assert_eq!(
        svc.credential_held(&session.id).await.as_deref(),
        Some(working.as_str()),
        "a refusal must leave the recorded working value unchanged"
    );
    assert!(!svc
        .can_reclaim_untracked_credential(&session.id, &["undelivered-replacement"])
        .await
        .expect("session store works"));
}

#[tokio::test]
async fn a_refused_subject_refresh_retains_the_runtime_s_prior_evidence() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (svc, refuse) = service_with_delivery_switch(sent);
    let owner = caller("subject-refresh-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;
    let working = svc
        .credential_held(&session.id)
        .await
        .expect("initial credential was acknowledged");

    refuse.store(true, Ordering::SeqCst);
    assert_eq!(
        svc.refresh_subject_credential(&owner.subject, "refused-renewal".into())
            .await,
        0
    );
    assert_eq!(
        svc.credential_held(&session.id).await.as_deref(),
        Some(working.as_str()),
        "refusal must retain evidence for the value the runtime still holds"
    );

    refuse.store(false, Ordering::SeqCst);
    assert_eq!(
        svc.refresh_subject_credential(&owner.subject, "accepted-renewal".into())
            .await,
        1
    );
    assert_eq!(
        svc.credential_held(&session.id).await.as_deref(),
        Some("accepted-renewal")
    );
}

#[tokio::test]
async fn a_delivered_credential_is_recorded_as_reclaim_evidence() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (svc, _refuse) = service_with_delivery_switch(sent);
    let owner = caller("delivery-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;

    assert!(
        svc.push_session_credential(&session.id, "delivered-replacement".into())
            .await
    );
    assert_eq!(
        svc.credential_held(&session.id).await.as_deref(),
        Some("delivered-replacement")
    );
    assert!(svc
        .can_reclaim_untracked_credential(&session.id, &["delivered-replacement"])
        .await
        .expect("session store works"));
}

#[tokio::test]
async fn a_durable_commit_failure_after_acknowledgement_keeps_prior_entry_evidence() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (svc, _refuse) = service_with_delivery_switch(Arc::clone(&sent));
    let owner = caller("commit-failure-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;
    let working = svc
        .credential_held(&session.id)
        .await
        .expect("initial credential was acknowledged");

    assert!(matches!(
        svc.push_session_credential_with(
            &session.id,
            "pending-b",
            |_target| async { Ok::<bool, crate::store::StoreError>(true) },
            |_target| async {
                Err(crate::store::StoreError::Unavailable(
                    "injected promotion failure".into(),
                ))
            },
        )
        .await,
        Err(crate::store::StoreError::Unavailable(_))
    ));
    assert!(credentials_seen(&sent)
        .await
        .contains(&"pending-b".to_string()));
    assert_eq!(
        svc.credential_held(&session.id).await.as_deref(),
        Some(working.as_str()),
        "entry evidence changes only after the durable callback succeeds"
    );
}

#[tokio::test]
async fn acknowledged_delivery_finishes_before_exact_h1_can_be_replaced() {
    let adapter = Arc::new(BlockingAdapter::default());
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let svc = service_with_adapter(adapter.clone(), store, None, true);
    let owner = caller("credential-first-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;
    let h1 = svc
        .entries
        .read()
        .await
        .get(&session.id)
        .and_then(|entry| entry.runtime.clone())
        .expect("H1 is installed");

    adapter.arm();
    let committed = Arc::new(AtomicBool::new(false));
    let delivery = tokio::spawn({
        let svc = Arc::clone(&svc);
        let id = session.id.clone();
        let committed = Arc::clone(&committed);
        async move {
            svc.push_session_credential_with(
                &id,
                "pending-b",
                |_target| async { Ok::<bool, crate::store::StoreError>(true) },
                move |_target| {
                    let committed = Arc::clone(&committed);
                    async move {
                        committed.store(true, Ordering::SeqCst);
                        Ok::<bool, crate::store::StoreError>(true)
                    }
                },
            )
            .await
        }
    });
    adapter.wait_until_entered().await;
    assert!(
        !committed.load(Ordering::SeqCst),
        "durable promotion cannot precede adapter acknowledgement"
    );

    let (attempting_tx, attempting_rx) = tokio::sync::oneshot::channel();
    let (replaced_tx, mut replaced_rx) = tokio::sync::oneshot::channel();
    let replacement = tokio::spawn({
        let svc = Arc::clone(&svc);
        let id = session.id.clone();
        let h1 = h1.clone();
        async move {
            let _ = attempting_tx.send(());
            let lifecycle = svc.lock_lifecycle(&id).await;
            let mut entries = svc.entries.write().await;
            let entry = entries.get_mut(&id).expect("H1 remains installed");
            assert_eq!(entry.runtime.as_ref(), Some(&h1));
            assert_eq!(entry.subject_credential.as_deref(), Some("pending-b"));
            entry.runtime = Some(layer3_adapter::RuntimeHandle("runtime-h2".into()));
            entry.subject_credential = None;
            drop(entries);
            drop(lifecycle);
            let _ = replaced_tx.send(());
        }
    });
    attempting_rx
        .await
        .expect("replacement task is contending for the lifecycle gate");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut replaced_rx)
            .await
            .is_err(),
        "replacement must wait while H1 acknowledgement is outstanding"
    );

    adapter.acknowledge();
    assert!(matches!(
        delivery.await.expect("delivery task joins"),
        Ok(true)
    ));
    replaced_rx
        .await
        .expect("replacement acquires the gate after evidence is recorded");
    replacement.await.expect("replacement task joins");
    assert!(committed.load(Ordering::SeqCst));
    assert!(adapter
        .sent
        .lock()
        .await
        .iter()
        .any(|(runtime, value)| runtime == &h1.0 && value == "pending-b"));
}

#[tokio::test]
async fn h1_removal_winning_the_gate_prevents_delivery_and_promotion_on_stale_h1() {
    let adapter = Arc::new(BlockingAdapter::default());
    let store = Arc::new(FakeStore::default());
    store.disable_coordination();
    let svc = service_with_adapter(adapter.clone(), store, None, true);
    let owner = caller("rotation-first-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;
    let sent_before = adapter.sent.lock().await.len();

    let lifecycle = svc.lock_lifecycle(&session.id).await;
    let mut h1_entry = svc
        .entries
        .write()
        .await
        .remove(&session.id)
        .expect("H1 is removed while holding the lifecycle gate");
    let current_a = h1_entry
        .subject_credential
        .clone()
        .expect("H1 acknowledged current A before removal");
    let promoted = Arc::new(AtomicBool::new(false));
    let delivery = tokio::spawn({
        let svc = Arc::clone(&svc);
        let id = session.id.clone();
        let promoted = Arc::clone(&promoted);
        async move {
            svc.push_session_credential_with(
                &id,
                "pending-b",
                |_target| async { Ok::<bool, crate::store::StoreError>(true) },
                move |_target| {
                    let promoted = Arc::clone(&promoted);
                    async move {
                        promoted.store(true, Ordering::SeqCst);
                        Ok::<bool, crate::store::StoreError>(true)
                    }
                },
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!delivery.is_finished(), "delivery waits for H1 removal");
    drop(lifecycle);

    assert!(matches!(
        delivery.await.expect("delivery task joins"),
        Ok(false)
    ));
    assert!(!promoted.load(Ordering::SeqCst));
    assert_eq!(adapter.sent.lock().await.len(), sent_before);

    let lifecycle = svc.lock_lifecycle(&session.id).await;
    h1_entry.runtime = Some(layer3_adapter::RuntimeHandle("runtime-h2".into()));
    h1_entry.subject_credential = Some(current_a.clone());
    svc.entries
        .write()
        .await
        .insert(session.id.clone(), h1_entry);
    drop(lifecycle);
    assert_eq!(
        svc.credential_held(&session.id).await,
        Some(current_a),
        "H2 retains the durable current value when B was never promoted"
    );
}

#[tokio::test]
async fn a_reused_handle_from_another_generation_receives_no_provisioning_credential() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(BlockingAdapter::default());
    let service = service_with_adapter(adapter.clone(), store, None, true);
    let owner = caller("credential-generation-owner");
    let session = service
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &session.id).await;
    let (runtime, generation) = {
        let entries = service.entries.read().await;
        let entry = entries.get(&session.id).expect("the runtime is installed");
        (
            entry.runtime.clone().expect("the runtime has a handle"),
            entry.runtime_generation,
        )
    };
    adapter.sent.lock().await.clear();

    let lifecycle = service.lock_lifecycle(&session.id).await;
    service
        .entries
        .write()
        .await
        .get_mut(&session.id)
        .expect("the same handle remains installed")
        .runtime_generation = generation.checked_add(1).expect("test generation advances");
    let committed = Arc::new(AtomicBool::new(false));
    let outcome = service
        .install_credential_while_lifecycle_locked(
            &lifecycle,
            &session.id,
            &runtime,
            generation,
            "stale-generation-value",
            {
                let committed = Arc::clone(&committed);
                move || async move {
                    committed.store(true, Ordering::SeqCst);
                    Ok::<bool, StoreError>(true)
                }
            },
        )
        .await;
    assert!(matches!(outcome, Ok(false)));
    assert!(!committed.load(Ordering::SeqCst));
    assert!(adapter.sent.lock().await.is_empty());
}

#[tokio::test]
async fn cross_replica_ack_and_promotion_finish_before_handover_can_replace_h1() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(BlockingAdapter::default());
    let credentials = SharedCredentialSource::new("current-a");
    let first = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let second = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let owner = caller("coordinated-credential-first-owner");
    let session = first
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&first, &owner, &session.id).await;
    let (h1, h1_generation) = {
        let entries = first.entries.read().await;
        let entry = entries.get(&session.id).expect("H1 is installed");
        (
            entry.runtime.clone().expect("H1 has a provider handle"),
            entry.runtime_generation,
        )
    };

    adapter.arm();
    let prepared_target = Arc::new(tokio::sync::Mutex::new(None));
    let promoted = Arc::new(AtomicBool::new(false));
    let commit_entered = Arc::new(Notify::new());
    let commit_release = Arc::new(Notify::new());
    let delivery = tokio::spawn({
        let first = Arc::clone(&first);
        let id = session.id.clone();
        let prepared_target = Arc::clone(&prepared_target);
        let promoted = Arc::clone(&promoted);
        let credentials = Arc::clone(&credentials);
        let commit_entered = Arc::clone(&commit_entered);
        let commit_release = Arc::clone(&commit_release);
        async move {
            first
                .push_session_credential_with(
                    &id,
                    "pending-b",
                    move |target| {
                        let prepared_target = Arc::clone(&prepared_target);
                        async move {
                            *prepared_target.lock().await = Some(target);
                            Ok::<bool, StoreError>(true)
                        }
                    },
                    move |_target| {
                        let promoted = Arc::clone(&promoted);
                        let credentials = Arc::clone(&credentials);
                        let commit_entered = Arc::clone(&commit_entered);
                        let commit_release = Arc::clone(&commit_release);
                        async move {
                            commit_entered.notify_one();
                            commit_release.notified().await;
                            credentials.replace("pending-b").await;
                            promoted.store(true, Ordering::SeqCst);
                            Ok::<bool, StoreError>(true)
                        }
                    },
                )
                .await
        }
    });
    adapter.wait_until_entered().await;

    assert!(matches!(
        acquire_handover(&second, &store, &session.id).await,
        Err(StoreError::SessionLeaseHeld { .. })
    ));
    assert!(!promoted.load(Ordering::SeqCst));
    assert_eq!(credentials.current().await, "current-a");

    adapter.acknowledge();
    tokio::time::timeout(Duration::from_secs(2), commit_entered.notified())
        .await
        .expect("promotion starts after H1 acknowledgement");
    assert!(matches!(
        acquire_handover(&second, &store, &session.id).await,
        Err(StoreError::SessionLeaseHeld { .. })
    ));
    assert!(!promoted.load(Ordering::SeqCst));
    commit_release.notify_one();
    assert!(matches!(
        delivery.await.expect("delivery task joins"),
        Ok(true)
    ));
    assert!(promoted.load(Ordering::SeqCst));
    assert_eq!(credentials.current().await, "pending-b");
    let target = prepared_target
        .lock()
        .await
        .clone()
        .expect("pending B was bound before delivery");
    assert_eq!(target.runtime_generation, h1_generation);
    assert_eq!(target.runtime_id, h1.0);
    assert_eq!(
        first.credential_held(&session.id).await.as_deref(),
        Some("pending-b")
    );

    wait_until_session_lease_released(&store, &session.id).await;
    let mut handover = acquire_handover(&second, &store, &session.id)
        .await
        .expect("B acquires handover only after A finalizes");
    let switched = switch_handover_to_h2(&store, &mut handover).await;
    assert_eq!(
        switched.boundary.authority.runtime_id.as_deref(),
        Some("runtime-h2")
    );
    store
        .release_session_lease(&handover)
        .await
        .expect("B releases completed handover");

    let h2 = store
        .get(&session.id)
        .await
        .expect("store works")
        .expect("session remains durable");
    second
        .readopt(&h2)
        .await
        .expect("the successor readopts H2");
    assert_eq!(
        second.credential_held(&session.id).await.as_deref(),
        Some("pending-b"),
        "H2 receives the value durably promoted after H1 acknowledgement"
    );
    assert!(adapter
        .sent
        .lock()
        .await
        .iter()
        .any(|(runtime, value)| runtime == "runtime-h2" && value == "pending-b"));
}

#[tokio::test]
async fn cross_replica_handover_winning_first_prevents_h1_delivery_and_promotion() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(BlockingAdapter::default());
    let credentials = SharedCredentialSource::new("current-a");
    let first = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let second = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let owner = caller("coordinated-handover-first-owner");
    let session = first
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&first, &owner, &session.id).await;

    let mut handover = acquire_handover(&second, &store, &session.id)
        .await
        .expect("B acquires a real handover lease");
    let switched = switch_handover_to_h2(&store, &mut handover).await;
    assert_eq!(
        switched.boundary.authority.runtime_id.as_deref(),
        Some("runtime-h2")
    );
    let sent_before = adapter.sent.lock().await.len();
    let prepared = Arc::new(AtomicBool::new(false));
    let promoted = Arc::new(AtomicBool::new(false));

    let delivered = first
        .push_session_credential_with(
            &session.id,
            "pending-b",
            {
                let prepared = Arc::clone(&prepared);
                move |_target| async move {
                    prepared.store(true, Ordering::SeqCst);
                    Ok::<bool, StoreError>(true)
                }
            },
            {
                let promoted = Arc::clone(&promoted);
                move |_target| async move {
                    promoted.store(true, Ordering::SeqCst);
                    Ok::<bool, StoreError>(true)
                }
            },
        )
        .await
        .expect("handover contention is a closed delivery result");
    assert!(!delivered);
    assert!(!prepared.load(Ordering::SeqCst));
    assert!(!promoted.load(Ordering::SeqCst));
    assert_eq!(adapter.sent.lock().await.len(), sent_before);
    assert_eq!(credentials.current().await, "current-a");

    store
        .release_session_lease(&handover)
        .await
        .expect("B releases completed handover");
    assert!(!first
        .push_session_credential_with(
            &session.id,
            "pending-b",
            {
                let prepared = Arc::clone(&prepared);
                move |_target| async move {
                    prepared.store(true, Ordering::SeqCst);
                    Ok::<bool, StoreError>(true)
                }
            },
            {
                let promoted = Arc::clone(&promoted);
                move |_target| async move {
                    promoted.store(true, Ordering::SeqCst);
                    Ok::<bool, StoreError>(true)
                }
            },
        )
        .await
        .expect("the stale local H1 entry is a closed mismatch"));
    assert!(!prepared.load(Ordering::SeqCst));
    assert!(!promoted.load(Ordering::SeqCst));
    assert_eq!(adapter.sent.lock().await.len(), sent_before);
    assert_eq!(
        first.credential_held(&session.id).await.as_deref(),
        Some("current-a"),
        "the stale H1 entry is not mutated after H2 becomes authoritative"
    );
    let h2 = store
        .get(&session.id)
        .await
        .expect("store works")
        .expect("session remains durable");
    second
        .readopt(&h2)
        .await
        .expect("the successor readopts H2");
    assert_eq!(
        second.credential_held(&session.id).await.as_deref(),
        Some("current-a"),
        "H2 retains A because B was never acknowledged or promoted on H1"
    );
    assert!(adapter
        .sent
        .lock()
        .await
        .iter()
        .any(|(runtime, value)| runtime == "runtime-h2" && value == "current-a"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_h1_finalizer_cannot_make_a_provisioned_h2_reinstall_current_a() {
    let store = Arc::new(FakeStore::default());
    let mut adapter_fixture = FakeAdapter::default();
    adapter_fixture.capabilities.conversation = true;
    let adapter = Arc::new(adapter_fixture);
    let credentials = CorrelatedCredentialSource::new("current-a");
    let first = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let second = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let owner = caller("expired-h1-provisioning-owner");
    let session = first
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("H1 is created");
    await_settled(&first, &owner, &session.id).await;
    let h1 = {
        let entries = first.entries.read().await;
        let entry = entries.get(&session.id).expect("H1 is installed");
        CredentialDeliveryTarget {
            runtime_generation: entry.runtime_generation,
            runtime_id: entry
                .runtime
                .as_ref()
                .expect("H1 has a provider handle")
                .0
                .clone(),
        }
    };
    assert_eq!(
        first.credential_held(&session.id).await.as_deref(),
        Some("current-a")
    );

    credentials.seed_pending("pending-b").await;
    adapter.credentials.lock().await.clear();
    credentials.block_next_confirmation();
    let h1_delivery = tokio::spawn({
        let first = Arc::clone(&first);
        let id = session.id.clone();
        let owner = owner.subject.clone();
        async move { first.supply_credential(&id, &owner).await }
    });
    credentials.wait_until_confirmation_paused().await;
    assert_eq!(
        adapter.credentials.lock().await.as_slice(),
        &["pending-b"],
        "H1 reached its post-acknowledgement finalizer with B"
    );
    let expired = store.expire_active_interaction(&session.id).await;
    assert_eq!(expired.boundary.authority.generation, h1.runtime_generation);
    assert_eq!(
        expired.boundary.authority.runtime_id.as_deref(),
        Some(h1.runtime_id.as_str())
    );

    let moving = second
        .change_plane(
            &owner,
            &session.id,
            crate::options::SessionMode::Conversation,
        )
        .await
        .expect("H2 acquires handover after the H1 interaction expires");
    assert_eq!(moving.status, crate::session::SessionStatus::Rotating);
    let settled = await_settled(&second, &owner, &session.id).await;
    assert_eq!(settled.status, crate::session::SessionStatus::Ready);
    let h2 = {
        let entries = second.entries.read().await;
        let entry = entries.get(&session.id).expect("H2 is installed");
        assert_eq!(entry.subject_credential.as_deref(), Some("pending-b"));
        CredentialDeliveryTarget {
            runtime_generation: entry.runtime_generation,
            runtime_id: entry
                .runtime
                .as_ref()
                .expect("H2 has a provider handle")
                .0
                .clone(),
        }
    };
    assert_ne!(h2, h1);
    assert!(
        adapter
            .last_spec
            .lock()
            .await
            .as_ref()
            .expect("H2 was provisioned from a runtime spec")
            .subject_credential
            .is_none(),
        "the replacement spec must not carry cached current A"
    );

    {
        let state = credentials.state.lock().await;
        assert_eq!(state.current, "pending-b");
        assert!(state.pending.is_none());
        assert!(state.pending_target.is_none());
        assert_eq!(state.revoke_debt, ["current-a"]);
        let b_targets: Vec<_> = state
            .preparations
            .iter()
            .filter(|(_, value)| value == "pending-b")
            .map(|(target, _)| target.clone())
            .collect();
        assert_eq!(
            b_targets,
            vec![h1.clone(), h2.clone()],
            "H2 retargets the exact pending B before provisioning delivery"
        );
    }
    assert_eq!(
        adapter.credentials.lock().await.as_slice(),
        &["pending-b", "pending-b"],
        "neither H1's overlap nor H2 provisioning may reinstall A"
    );

    credentials.resume_confirmation();
    h1_delivery.await.expect("the old H1 finalizer completes");
    let state = credentials.state.lock().await;
    assert_eq!(state.current, "pending-b");
    assert!(state.pending.is_none());
    assert_eq!(
        state.revoke_debt,
        ["current-a"],
        "H1's late idempotent confirmation cannot authorize or queue A again"
    );
    assert_eq!(
        state
            .confirmations
            .iter()
            .filter(|(_, value, confirmed)| value == "pending-b" && *confirmed)
            .count(),
        2,
        "H2 promotion and H1's late idempotent completion both confirm B"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_h1_finalizer_cannot_make_a_readopted_h2_reinstall_current_a() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let credentials = CorrelatedCredentialSource::new("current-a");
    let first = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let second = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let owner = caller("expired-h1-readoption-owner");
    let session = first
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("H1 is created");
    await_settled(&first, &owner, &session.id).await;
    let h1 = {
        let entries = first.entries.read().await;
        let entry = entries.get(&session.id).expect("H1 is installed");
        CredentialDeliveryTarget {
            runtime_generation: entry.runtime_generation,
            runtime_id: entry
                .runtime
                .as_ref()
                .expect("H1 has a provider handle")
                .0
                .clone(),
        }
    };

    credentials.seed_pending("pending-b").await;
    adapter.credentials.lock().await.clear();
    credentials.block_next_confirmation();
    let h1_delivery = tokio::spawn({
        let first = Arc::clone(&first);
        let id = session.id.clone();
        let owner = owner.subject.clone();
        async move { first.supply_credential(&id, &owner).await }
    });
    credentials.wait_until_confirmation_paused().await;
    store.expire_active_interaction(&session.id).await;

    let mut handover = acquire_handover(&second, &store, &session.id)
        .await
        .expect("H2 authority can be installed after H1 expires");
    let switched = switch_handover_to_h2(&store, &mut handover).await;
    let h2 = CredentialDeliveryTarget {
        runtime_generation: switched.boundary.authority.generation,
        runtime_id: switched
            .boundary
            .authority
            .runtime_id
            .clone()
            .expect("H2 authority has a provider handle"),
    };
    adapter
        .running
        .lock()
        .await
        .insert(h2.runtime_id.clone(), Some(session.id.as_str().to_string()));
    adapter.generations.lock().await.insert(
        h2.runtime_id.clone(),
        std::num::NonZeroU64::new(h2.runtime_generation)
            .expect("coordinated runtime generations are non-zero"),
    );
    store
        .release_session_lease(&handover)
        .await
        .expect("the authority switch releases handover");

    let durable_h2 = store
        .get(&session.id)
        .await
        .expect("the store remains readable")
        .expect("the session remains durable");
    second
        .readopt(&durable_h2)
        .await
        .expect("the successor readopts exact H2");
    {
        let entries = second.entries.read().await;
        let entry = entries.get(&session.id).expect("readopted H2 is live");
        assert_eq!(entry.runtime_generation, h2.runtime_generation);
        assert_eq!(
            entry.runtime.as_ref().map(|runtime| runtime.0.as_str()),
            Some(h2.runtime_id.as_str())
        );
        assert_eq!(entry.subject_credential.as_deref(), Some("pending-b"));
    }
    {
        let state = credentials.state.lock().await;
        assert_eq!(state.current, "pending-b");
        assert!(state.pending.is_none());
        assert!(state.pending_target.is_none());
        assert_eq!(state.revoke_debt, ["current-a"]);
        let b_targets: Vec<_> = state
            .preparations
            .iter()
            .filter(|(_, value)| value == "pending-b")
            .map(|(target, _)| target.clone())
            .collect();
        assert_eq!(b_targets, vec![h1, h2]);
    }
    assert_eq!(
        adapter.credentials.lock().await.as_slice(),
        &["pending-b", "pending-b"],
        "recovery top-up selects pending B rather than ordinary current A"
    );

    credentials.resume_confirmation();
    h1_delivery.await.expect("the old H1 finalizer completes");
    let state = credentials.state.lock().await;
    assert_eq!(state.current, "pending-b");
    assert_eq!(state.revoke_debt, ["current-a"]);
}

#[tokio::test]
async fn caller_cancellation_after_delivery_starts_cannot_abandon_acknowledged_promotion() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(BlockingAdapter::default());
    let credentials = SharedCredentialSource::new("current-a");
    let service = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let owner = caller("cancelled-credential-caller");
    let session = service
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&service, &owner, &session.id).await;

    adapter.arm();
    let committed = Arc::new(Notify::new());
    let caller = tokio::spawn({
        let service = Arc::clone(&service);
        let id = session.id.clone();
        let credentials = Arc::clone(&credentials);
        let committed = Arc::clone(&committed);
        async move {
            service
                .push_session_credential_with(
                    &id,
                    "pending-b",
                    |_target| async { Ok::<bool, StoreError>(true) },
                    move |_target| async move {
                        credentials.replace("pending-b").await;
                        committed.notify_one();
                        Ok::<bool, StoreError>(true)
                    },
                )
                .await
        }
    });
    adapter.wait_until_entered().await;
    caller.abort();
    assert!(caller
        .await
        .expect_err("the waiting caller is cancelled")
        .is_cancelled());

    adapter.acknowledge();
    tokio::time::timeout(Duration::from_secs(2), committed.notified())
        .await
        .expect("post-acknowledgement promotion outlives its caller");
    tokio::time::timeout(Duration::from_secs(2), async {
        while service.credential_held(&session.id).await.as_deref() != Some("pending-b") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact H1 evidence is recorded after caller cancellation");
    assert_eq!(credentials.current().await, "pending-b");
    wait_until_session_lease_released(&store, &session.id).await;
}

#[tokio::test]
async fn recovery_selection_holds_interaction_authority_until_delivery_finishes() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(BlockingAdapter::default());
    let credentials = SharedCredentialSource::new("current-a");
    let first = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let second = service_with_adapter_and_credentials(
        adapter.clone(),
        Arc::clone(&store),
        credentials.clone(),
    );
    let owner = caller("stale-recovery-selection-owner");
    let session = first
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&first, &owner, &session.id).await;
    let durable = store
        .get(&session.id)
        .await
        .expect("store works")
        .expect("session remains durable");
    second
        .readopt(&durable)
        .await
        .expect("the second replica readopts H1");

    first
        .entries
        .write()
        .await
        .get_mut(&session.id)
        .expect("the first replica still has H1")
        .subject_credential = None;
    adapter.sent.lock().await.clear();
    credentials.pause_next_read();
    let stale_supply = tokio::spawn({
        let first = Arc::clone(&first);
        let id = session.id.clone();
        let owner = owner.subject.clone();
        async move { first.supply_credential(&id, &owner).await }
    });
    credentials.wait_until_read_paused().await;

    let blocked = second
        .push_session_credential_with(
            &session.id,
            "pending-b",
            |_target| async { Ok::<bool, StoreError>(true) },
            {
                let credentials = Arc::clone(&credentials);
                move |_target| async move {
                    credentials.replace("pending-b").await;
                    Ok::<bool, StoreError>(true)
                }
            },
        )
        .await;
    assert!(matches!(
        blocked,
        Err(StoreError::Unavailable(detail)) if detail.contains("blocked by Interact")
    ));
    assert_eq!(credentials.current().await, "current-a");
    assert!(
        adapter.sent.lock().await.is_empty(),
        "the paused selection has not reached the runtime"
    );

    credentials.resume_read();
    stale_supply.await.expect("stale supply task joins");
    assert_eq!(
        first.credential_held(&session.id).await.as_deref(),
        Some("current-a"),
        "the selected A is delivered while its interaction still owns ordering"
    );
    wait_until_session_lease_released(&store, &session.id).await;

    assert!(second
        .push_session_credential_with(
            &session.id,
            "pending-b",
            |_target| async { Ok::<bool, StoreError>(true) },
            {
                let credentials = Arc::clone(&credentials);
                move |_target| async move {
                    credentials.replace("pending-b").await;
                    Ok::<bool, StoreError>(true)
                }
            },
        )
        .await
        .expect("B delivery follows the completed A interaction"));
    assert_eq!(credentials.current().await, "pending-b");
    assert_eq!(
        second.credential_held(&session.id).await.as_deref(),
        Some("pending-b")
    );
    let sent = adapter.sent.lock().await;
    assert_eq!(
        sent.iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>(),
        vec!["current-a", "pending-b"],
        "A cannot be delivered after the interaction that promotes B"
    );
}

#[tokio::test]
async fn a_failed_top_up_stays_missing_until_delivery_can_be_retried() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let refuse = Arc::new(AtomicBool::new(true));
    let svc = service_with_store_and_refusal(
        Arc::clone(&sent),
        Arc::new(FakeStore::default()),
        None,
        true,
        Arc::clone(&refuse),
    );
    let owner = caller("recovered-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;

    assert_eq!(svc.credential_held(&session.id).await, None);
    assert!(
        credentials_seen(&sent).await.is_empty(),
        "an intended initial value is not evidence when the adapter refuses it"
    );
    assert_eq!(svc.top_up_credentials().await, 0);
    assert_eq!(svc.credential_held(&session.id).await, None);
    let recovered = format!("minted-for-{}", session.id);
    assert!(!svc
        .can_reclaim_untracked_credential(&session.id, &[recovered.as_str()])
        .await
        .expect("session store works"));

    refuse.store(false, Ordering::SeqCst);
    assert_eq!(svc.top_up_credentials().await, 1);
    assert_eq!(svc.credential_held(&session.id).await, Some(recovered));
}

#[tokio::test]
async fn readoption_records_no_credential_until_the_reconnected_runtime_acknowledges_one() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (svc, refuse) = service_with_delivery_switch(Arc::clone(&sent));
    let owner = caller("readoption-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;
    let record = svc
        .record(&session.id)
        .await
        .expect("record remains durable");
    svc.entries.write().await.remove(&session.id);
    sent.lock().await.clear();
    refuse.store(true, Ordering::SeqCst);

    svc.readopt(&record)
        .await
        .expect("the existing runtime is reconnected");
    assert_eq!(svc.credential_held(&session.id).await, None);
    assert!(credentials_seen(&sent).await.is_empty());

    refuse.store(false, Ordering::SeqCst);
    assert_eq!(svc.top_up_credentials().await, 1);
    assert_eq!(
        svc.credential_held(&session.id).await,
        Some(format!("minted-for-{}", session.id))
    );
}

/// Which sessions still have a claim on the credential minted for them.
///
/// The reclaim sweep asks this before taking a key back, and asking the wrong
/// question revoked the key of every paused session as soon as the process
/// holding the in-memory record of it restarted.
mod what_still_claims_a_credential {
    use super::*;

    #[tokio::test]
    async fn a_paused_session_still_claims_its_credential() {
        // Its owner stopped it and can pick it up again. Taking the key back
        // leaves them returning to a session whose credential was revoked
        // underneath them.
        let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let svc = service(Arc::clone(&sent));
        let owner = caller("subject-a");
        let session = svc
            .create_session(&owner, crate::test_support::spec())
            .await
            .expect("created");
        await_settled(&svc, &owner, &session.id).await;

        svc.stop_session(&owner, &session.id)
            .await
            .expect("a session can be paused");

        assert!(
            svc.session_claims_credential(&session.id)
                .await
                .expect("session store works"),
            "a paused session keeps its claim"
        );
        assert!(
            !svc.session_is_live(&session.id).await,
            "though it wants no runtime, which is the answer that used to be asked"
        );
        assert!(svc
            .can_reclaim_untracked_credential(&session.id, &[])
            .await
            .expect("session store works"));
    }

    #[tokio::test]
    async fn a_terminated_session_claims_nothing() {
        let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let svc = service(Arc::clone(&sent));
        let owner = caller("subject-a");
        let session = svc
            .create_session(&owner, crate::test_support::spec())
            .await
            .expect("created");
        await_settled(&svc, &owner, &session.id).await;

        svc.terminate_session(&owner, &session.id)
            .await
            .expect("a session can be ended");

        assert!(
            !svc.session_claims_credential(&session.id)
                .await
                .expect("session store works"),
            "an ended session has no claim, so its key is the sweep's to take"
        );
    }

    #[tokio::test]
    async fn a_session_that_does_not_exist_claims_nothing() {
        let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let svc = service(Arc::clone(&sent));
        assert!(
            !svc.session_claims_credential(&crate::ids::SessionId("sesn_never".into()))
                .await
                .expect("session store works"),
            "a key naming a session nobody has is exactly what the sweep exists for"
        );
        assert!(svc
            .can_reclaim_untracked_credential(&crate::ids::SessionId("sesn_never".into()), &[],)
            .await
            .expect("session store works"));
    }

    #[tokio::test]
    async fn a_store_outage_is_not_misreported_as_an_unclaimed_credential() {
        let store = Arc::new(FakeStore::default());
        let svc = service_with_store(
            Arc::new(tokio::sync::Mutex::new(Vec::new())),
            Arc::clone(&store),
            None,
            true,
        );
        store.fail_everything().await;

        assert!(matches!(
            svc.session_claims_credential(&crate::ids::SessionId("sesn_unreadable".into()))
                .await,
            Err(crate::store::StoreError::Unavailable(_))
        ));
        assert!(matches!(
            svc.can_reclaim_untracked_credential(
                &crate::ids::SessionId("sesn_unreadable".into()),
                &[],
            )
            .await,
            Err(crate::store::StoreError::Unavailable(_))
        ));
    }
}

/// The durable authority used when recurring work replaces a session key.
mod recurring_credential_context {
    use super::*;

    #[tokio::test]
    async fn an_active_session_returns_its_durable_owner_and_frozen_grant() {
        let store = Arc::new(FakeStore::default());
        let svc = service_with_store(
            Arc::new(tokio::sync::Mutex::new(Vec::new())),
            Arc::clone(&store),
            None,
            true,
        );
        let owner = caller("durable-owner");
        let session = svc
            .create_session(&owner, crate::test_support::spec())
            .await
            .expect("session is durably created");

        let (recorded_owner, services) = svc
            .session_credential_context(&session.id)
            .await
            .expect("session store works")
            .expect("active session has credential context");
        assert_eq!(recorded_owner, owner.subject);
        assert_eq!(
            services,
            store
                .sessions
                .lock()
                .await
                .get(session.id.as_str())
                .expect("durable session exists")
                .session
                .granted_services
                .clone()
                .expect("new sessions freeze their service grant")
        );
    }

    #[tokio::test]
    async fn missing_and_terminal_sessions_have_no_credential_context() {
        let svc = service(Arc::new(tokio::sync::Mutex::new(Vec::new())));
        assert_eq!(
            svc.session_credential_context(&SessionId("sesn_missing".into()))
                .await
                .expect("session store works"),
            None
        );

        let owner = caller("terminal-owner");
        let session = svc
            .create_session(&owner, crate::test_support::spec())
            .await
            .expect("session is created");
        svc.terminate_session(&owner, &session.id)
            .await
            .expect("session is terminated");
        assert_eq!(
            svc.session_credential_context(&session.id)
                .await
                .expect("session store works"),
            None
        );
    }

    #[tokio::test]
    async fn a_store_outage_is_not_misreported_as_missing_context() {
        let store = Arc::new(FakeStore::default());
        let svc = service_with_store(
            Arc::new(tokio::sync::Mutex::new(Vec::new())),
            Arc::clone(&store),
            None,
            true,
        );
        store.fail_everything().await;

        assert!(matches!(
            svc.session_credential_context(&SessionId("sesn_unreadable".into()))
                .await,
            Err(crate::store::StoreError::Unavailable(_))
        ));
    }
}
