use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Journal {
    refuse: AtomicBool,
    fail_outcome: AtomicBool,
    admitted: AtomicUsize,
    outcomes: Mutex<Vec<ObservedOutcome>>,
}

struct Permit(Arc<Journal>);

#[async_trait::async_trait]
impl EffectPermit for Permit {
    async fn record_outcome(&self, outcome: ObservedOutcome) -> Result<()> {
        if self.0.fail_outcome.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "outcome journal unavailable".into(),
            ));
        }
        self.0.outcomes.lock().unwrap().push(outcome);
        Ok(())
    }
}

struct Authority(Arc<Journal>);

#[async_trait::async_trait]
impl EffectAuthority for Authority {
    async fn admit(&self, _: &EffectIntent) -> Result<Box<dyn EffectPermit>> {
        if self.0.refuse.load(Ordering::SeqCst) {
            return Err(AdapterError::Rejected("admission is frozen".into()));
        }
        self.0.admitted.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Permit(self.0.clone())))
    }
}

fn authority(journal: &Arc<Journal>) -> Arc<dyn EffectAuthority> {
    Arc::new(Authority(journal.clone()))
}

fn intent() -> EffectIntent {
    EffectIntent::new("claim", "logical-generation", &vec![1_u8, 2, 3]).unwrap()
}

#[tokio::test]
async fn freeze_refuses_before_polling_provider() {
    let journal = Arc::new(Journal::default());
    journal.refuse.store(true, Ordering::SeqCst);
    let dispatched = AtomicBool::new(false);
    let result = dispatch(&authority(&journal), intent(), async {
        dispatched.store(true, Ordering::SeqCst);
        Ok(())
    })
    .await;
    assert!(result.is_err());
    assert!(!dispatched.load(Ordering::SeqCst));
    assert_eq!(journal.admitted.load(Ordering::SeqCst), 0);
    assert!(journal.outcomes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn acknowledgement_is_observed_without_settling_the_permit() {
    let journal = Arc::new(Journal::default());
    let value = dispatch(&authority(&journal), intent(), async { Ok(17) })
        .await
        .unwrap();
    assert_eq!(value, 17);
    assert_eq!(journal.admitted.load(Ordering::SeqCst), 1);
    assert_eq!(
        *journal.outcomes.lock().unwrap(),
        vec![ObservedOutcome::Acknowledged]
    );
}

#[tokio::test]
async fn provider_error_is_an_unknown_effect_even_after_caller_returns() {
    let journal = Arc::new(Journal::default());
    let result: Result<()> = dispatch(&authority(&journal), intent(), async {
        Err(AdapterError::Unavailable("acknowledgement lost".into()))
    })
    .await;
    assert!(result.is_err());
    assert_eq!(journal.admitted.load(Ordering::SeqCst), 1);
    assert_eq!(
        *journal.outcomes.lock().unwrap(),
        vec![ObservedOutcome::Unknown]
    );
}

#[tokio::test]
async fn cancellation_retains_admission_without_a_success_outcome() {
    let journal = Arc::new(Journal::default());
    let gate = authority(&journal);
    let (started, observed) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        dispatch(&gate, intent(), async {
            started.send(()).unwrap();
            std::future::pending::<Result<()>>().await
        })
        .await
    });
    observed.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(journal.admitted.load(Ordering::SeqCst), 1);
    assert!(journal.outcomes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_outcome_persistence_cannot_return_provider_success() {
    let journal = Arc::new(Journal::default());
    journal.fail_outcome.store(true, Ordering::SeqCst);
    assert!(
        dispatch(&authority(&journal), intent(), async { Ok(()) })
            .await
            .is_err()
    );
    assert_eq!(journal.admitted.load(Ordering::SeqCst), 1);
    assert!(journal.outcomes.lock().unwrap().is_empty());
}

#[test]
fn intent_validates_safe_identity_and_persists_only_request_digest() {
    let request = serde_json::json!({"credential_payload": uuid::Uuid::new_v4().to_string()});
    let mut request_intent = EffectIntent::new("claim", "original-resource", &request).unwrap();
    let encoded = serde_json::to_string(&request_intent).unwrap();
    assert!(!encoded.contains(request["credential_payload"].as_str().unwrap()));
    request_intent.resource.push('\n');
    assert!(request_intent.validate().is_err());
    request_intent.resource = "original-resource".into();
    request_intent.request_sha256 = "unknown".into();
    assert!(request_intent.validate().is_err());
}
