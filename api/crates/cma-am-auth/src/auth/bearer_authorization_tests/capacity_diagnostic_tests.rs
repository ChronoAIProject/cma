use super::*;
use crate::auth::RequestAuth;
use layer2_core::{diagnostic_context::DiagnosticContext, StoreError};
use serde_json::Value;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Log(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn capture<F: std::future::Future>(future: F) -> (F::Output, Vec<Value>) {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .event_format(crate::diagnostic_format::json())
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let context = DiagnosticContext {
        request_hash: "synthetic-capacity-request".into(),
        ..DiagnosticContext::default()
    };
    let result = context.scope(future).with_subscriber(subscriber).await;
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let events = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["fields"]["event"] == "identity_verification_failure")
        .collect();
    (result, events)
}

fn assert_refusal(result: Result<Option<RequestAuth>, StoreError>) {
    assert!(matches!(result, Err(StoreError::Unavailable(message))
        if message == "bearer authorization could not be verified"));
}

fn assert_admission(events: &[Value], reason: &str) {
    assert_eq!(events.len(), 1, "{events:?}");
    let fields = &events[0]["fields"];
    assert_eq!(fields["request_hash"], "synthetic-capacity-request");
    assert_eq!(fields["endpoint"], "verification");
    assert_eq!(fields["stage"], "admission");
    assert_eq!(fields["reason"], reason);
    assert_eq!(fields["disposition"], "unavailable");
    assert!(fields["status"].is_null());
    for private in [
        "fixture-private",
        "fixture-subject",
        "fixture-secret",
        "http://",
    ] {
        assert!(!events[0].to_string().contains(private));
    }
}

#[tokio::test]
async fn sixty_fifth_subject_is_refused_without_provider_io_and_recovers_after_release() {
    let f = Fixture::new().await;
    f.control.token_subjects.store(true, Ordering::SeqCst);
    let token = "fixture-private-previously-verified";
    assert!(identity::request_for_bearer(&f.auth, token.into())
        .await
        .unwrap()
        .is_some());
    f.arrived(1).await;
    let gate = f.block().await;
    let mut admitted = tokio::task::JoinSet::new();
    for n in 0..64 {
        let auth = f.auth.clone();
        admitted.spawn(async move {
            let token = format!("fixture-private-inflight-{n}");
            let result = identity::request_for_bearer(&auth, token.clone()).await;
            (token, result)
        });
    }
    f.arrived(64).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.control.userinfo_calls.load(Ordering::SeqCst) != 65 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.auth.verification_slots.available_permits(), 0);
    let (refused, events) = capture(identity::request_for_bearer(&f.auth, token.into())).await;
    assert_refusal(refused);
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 65);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 65);
    assert_eq!(f.auth.verification_slots.available_permits(), 0);

    gate.release.add_permits(64);
    while let Some(result) = admitted.join_next().await {
        let (token, result) = result.unwrap();
        let request = result.unwrap().unwrap();
        assert_eq!(request.subject.as_str(), format!("fixture-subject-{token}"));
        assert!(request
            .authorization
            .allows(layer2_core::Capability::UseOwnSandboxes));
    }
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    assert!(f.auth.bearer_verifications.entries.lock().await.is_empty());
    *f.control.gate.write().await = None;
    assert!(identity::request_for_bearer(&f.auth, token.into())
        .await
        .unwrap()
        .is_some());
    f.control.active.store(false, Ordering::SeqCst);
    assert!(identity::request_for_bearer(&f.auth, token.into())
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 67);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 67);
    assert_admission(&events, "verification_capacity");
}

#[tokio::test]
async fn token_fence_capacity_is_distinct_and_returns_the_verification_permit() {
    let f = Fixture::new().await;
    let mut epochs: Vec<_> = (0..1024)
        .map(|n| {
            let key = f
                .auth
                .identity_store
                .key(&format!("fixture-private-held-{n}"));
            f.auth.identity_store.token_epoch(&key).unwrap()
        })
        .collect();
    let token = "fixture-private-fence-overflow";
    let (refused, events) = capture(identity::request_for_bearer(&f.auth, token.into())).await;
    assert_refusal(refused);
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    assert!(f.auth.bearer_verifications.entries.lock().await.is_empty());
    drop(epochs.pop());
    assert!(identity::request_for_bearer(&f.auth, token.into())
        .await
        .unwrap()
        .is_some());
    f.control.active.store(false, Ordering::SeqCst);
    assert!(identity::request_for_bearer(&f.auth, token.into())
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.auth.verification_slots.available_permits(), 64);
    assert_admission(&events, "verification_fence_capacity");
}
