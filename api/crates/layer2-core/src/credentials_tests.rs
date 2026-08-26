//! Tests for delegated-credential renewal.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::KernelAdapter;

use crate::authz::{Authorization, Caller, Capability};
use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::SessionService;
use crate::session::SessionSpec;
use crate::terminal::ScreenModel;
use crate::test_support::{await_settled, FakeStore, NullScreen};

/// A runtime that records the credentials it was handed, so the service's
/// behaviour is observable without a real sandbox.
///
/// It records them at `refresh_credential` rather than by watching terminal
/// input, which is the point of these tests now: a credential reaches a
/// runtime because the adapter was asked for it, not because it happened to
/// share a channel with keystrokes.
struct RecordingAdapter {
    sent: Arc<tokio::sync::Mutex<Vec<String>>>,
    refuse: Arc<std::sync::atomic::AtomicBool>,
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
        // Drained rather than inspected: what this adapter is watched for now
        // travels through `refresh_credential`, and an unread receiver would
        // make the service's terminal writes fail for reasons unrelated to
        // what is under test.
        let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<layer3_adapter::TerminalInput>(16);
        let (_out_tx, out_rx) = tokio::sync::mpsc::channel::<layer3_adapter::TerminalOutput>(16);
        tokio::spawn(async move { while in_rx.recv().await.is_some() {} });
        Ok(layer3_adapter::TerminalChannel {
            output: out_rx,
            input: in_tx,
        })
    }

    async fn refresh_credential(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
        credential: &str,
    ) -> Result<(), layer3_adapter::AdapterError> {
        if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Unavailable(
                "the runtime did not take the credential".into(),
            ));
        }
        self.sent.lock().await.push(credential.to_string());
        Ok(())
    }

    async fn terminate(
        &self,
        _handle: &layer3_adapter::RuntimeHandle,
    ) -> Result<(), layer3_adapter::AdapterError> {
        Ok(())
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
    refuse: Arc<std::sync::atomic::AtomicBool>,
) -> Arc<SessionService> {
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(
        KernelId::new("recording"),
        Arc::new(RecordingAdapter { sent, refuse }),
    );
    // A real store rather than a stub: these tests care that a credential
    // reaches the runtime, and that path now runs through persistence.
    SessionService::new(
        adapters,
        Arc::new(|g| Box::new(NullScreen(g)) as Box<dyn ScreenModel>),
        store,
        crate::service::SessionPorts {
            credentials: Some(Arc::new(MintingSource {
                observed_callers,
                tracks_subject,
            })),
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

fn service_with_delivery_switch(
    sent: Arc<tokio::sync::Mutex<Vec<String>>>,
) -> (Arc<SessionService>, Arc<std::sync::atomic::AtomicBool>) {
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
    assert_eq!(
        *observed_callers.lock().await,
        vec![Some("first-token".to_string())],
        "the credential source needs the initiating request's authority to mint custody"
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
async fn a_failed_top_up_stays_missing_until_delivery_can_be_retried() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let (svc, refuse) = service_with_delivery_switch(sent);
    let owner = caller("recovered-owner");
    let session = svc
        .create_session(&owner, crate::test_support::spec())
        .await
        .expect("session is created");
    await_settled(&svc, &owner, &session.id).await;

    svc.entries
        .write()
        .await
        .get_mut(&session.id)
        .expect("the running session has an entry")
        .subject_credential = None;
    refuse.store(true, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(svc.top_up_credentials().await, 0);
    assert_eq!(svc.credential_held(&session.id).await, None);
    let recovered = format!("minted-for-{}", session.id);
    assert!(!svc
        .can_reclaim_untracked_credential(&session.id, &[recovered.as_str()])
        .await
        .expect("session store works"));

    refuse.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(svc.top_up_credentials().await, 1);
    assert_eq!(svc.credential_held(&session.id).await, Some(recovered));
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
