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
        let (_out_tx, out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
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
        None,
        None,
        crate::environment_service::EnvironmentProfileService::new(
            Arc::new(crate::test_support::FakeProfileStore::default()),
            crate::environment::Baseline::default(),
            None,
            None,
            crate::environment::AgentConfiguration {
                kernel: KernelId::new("recording"),
                ..Default::default()
            },
        ),
        crate::test_support::owned(),
    )
}

async fn credentials_seen(sent: &Arc<tokio::sync::Mutex<Vec<String>>>) -> Vec<String> {
    sent.lock().await.clone()
}

#[tokio::test]
async fn the_owner_credential_reaches_the_runtime_at_creation() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service(Arc::clone(&sent));
    let owner = caller("subject-a");
    svc.create_session(
        &owner,
        SessionSpec {
            environment_profile: None,
            geometry: None,
            options: Default::default(),
            subject_credential: Some("first-token".into()),
        },
    )
    .await
    .expect("session created");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        credentials_seen(&sent).await,
        vec!["first-token".to_string()]
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
                subject_credential: Some("first-token".into()),
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
    assert_eq!(
        credentials_seen(&sent).await,
        vec!["first-token".to_string(), "second-token".to_string()]
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
            subject_credential: Some("a-token".into()),
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
                subject_credential: Some("token".into()),
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
async fn a_session_created_without_a_credential_is_still_usable() {
    let sent = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let svc = service(Arc::clone(&sent));
    let owner = caller("subject-a");
    svc.create_session(
        &owner,
        SessionSpec {
            environment_profile: None,
            geometry: None,
            subject_credential: None,
            options: Default::default(),
        },
    )
    .await
    .expect("a session with no delegation still starts");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(credentials_seen(&sent).await.is_empty());
}
