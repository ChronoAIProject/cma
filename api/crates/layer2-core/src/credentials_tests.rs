//! Tests for delegated-credential renewal.

use std::collections::HashMap;
use std::sync::Arc;

use layer3_adapter::KernelAdapter;

use crate::authz::{Authorization, Caller, Capability};
use crate::ids::{KernelId, SubjectId};
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
struct MintingSource;

#[async_trait::async_trait]
impl crate::store::CredentialSource for MintingSource {
    async fn credential_for(
        &self,
        session: &crate::SessionId,
        _subject: &crate::SubjectId,
        _services: &[crate::environment::ServiceId],
    ) -> Option<String> {
        Some(format!("minted-for-{}", session.0))
    }

    /// Nothing to reclaim: this mints a value rather than registering one.
    async fn release(&self, _session: &crate::SessionId) {}
}

fn service(sent: Arc<tokio::sync::Mutex<Vec<String>>>) -> Arc<SessionService> {
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(
        KernelId::new("recording"),
        Arc::new(RecordingAdapter { sent }),
    );
    // A real store rather than a stub: these tests care that a credential
    // reaches the runtime, and that path now runs through persistence.
    SessionService::new(
        adapters,
        Arc::new(|g| Box::new(NullScreen(g)) as Box<dyn ScreenModel>),
        Arc::new(FakeStore::default()),
        crate::service::SessionPorts {
            credentials: Some(Arc::new(MintingSource)),
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
            svc.session_claims_credential(&session.id).await,
            "a paused session keeps its claim"
        );
        assert!(
            !svc.session_is_live(&session.id).await,
            "though it wants no runtime, which is the answer that used to be asked"
        );
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
            !svc.session_claims_credential(&session.id).await,
            "an ended session has no claim, so its key is the sweep's to take"
        );
    }

    #[tokio::test]
    async fn a_session_that_does_not_exist_claims_nothing() {
        let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let svc = service(Arc::clone(&sent));
        assert!(
            !svc.session_claims_credential(&crate::ids::SessionId("sesn_never".into()))
                .await,
            "a key naming a session nobody has is exactly what the sweep exists for"
        );
    }
}
