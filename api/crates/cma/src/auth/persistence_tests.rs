use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::*;

fn subject_state(revision: u64) -> SubjectAuthState {
    SubjectAuthState {
        subject: SubjectId("subject-a".into()),
        revision,
        tokens: None,
        refresh_lease: None,
        authorization: Authorization::none(),
        profile: Profile::default(),
    }
}

fn credential_state(generation: u64) -> SessionCredentialState {
    SessionCredentialState {
        session: SessionId("sesn_a".into()),
        generation,
        current: None,
        pending: None,
        revoke_debt: Vec::new(),
        lease: None,
        retired: false,
    }
}

#[test]
fn secret_diagnostics_are_redacted() {
    let secret = SecretValue::new("plaintext-under-test".into());
    let rendered = format!("{secret:?}");
    assert!(rendered.contains("redacted"));
    assert!(!rendered.contains(secret.expose()));
}

#[test]
fn record_domains_produce_distinct_identifiers() {
    let value = "opaque-random-value";
    assert_ne!(
        SecretDigest::identify("browser-session", value),
        SecretDigest::identify("login-attempt", value)
    );
}

#[tokio::test]
async fn two_instances_share_browser_sessions_and_consume_login_once() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = store.clone() as Arc<dyn AuthStore>;
    let second = store as Arc<dyn AuthStore>;
    let browser_id = SecretDigest::identify("browser-session", "cookie");
    first
        .put_browser_session(BrowserSession {
            id: browser_id.clone(),
            subject: SubjectId("subject-a".into()),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        })
        .await
        .unwrap();
    assert!(second.browser_session(&browser_id).await.unwrap().is_some());

    let login_id = SecretDigest::identify("login-attempt", "state");
    first
        .put_login_attempt(LoginAttempt {
            id: login_id.clone(),
            verifier: SecretValue::new("verifier".into()),
            expires_at: SystemTime::now() + Duration::from_secs(60),
        })
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        first.take_login_attempt(&login_id),
        second.take_login_attempt(&login_id)
    );
    let winners = [left.unwrap(), right.unwrap()]
        .into_iter()
        .filter(Option::is_some)
        .count();
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn subject_compare_and_swap_has_one_winner() {
    let store = Arc::new(MemoryAuthStore::default());
    assert!(store.insert_subject(subject_state(0)).await.unwrap());
    let (left, right) = tokio::join!(
        store.replace_subject(0, subject_state(1)),
        store.replace_subject(0, subject_state(1))
    );
    assert_eq!(
        [left.unwrap(), right.unwrap()]
            .into_iter()
            .filter(|won| *won)
            .count(),
        1
    );
}

#[tokio::test]
async fn credential_compare_and_swap_rejects_a_stale_worker() {
    let store = MemoryAuthStore::default();
    assert!(store.insert_credential(credential_state(0)).await.unwrap());
    assert!(store
        .replace_credential(0, credential_state(1))
        .await
        .unwrap());
    assert!(!store
        .replace_credential(0, credential_state(1))
        .await
        .unwrap());
    assert_eq!(
        store
            .credential(&SessionId("sesn_a".into()))
            .await
            .unwrap()
            .unwrap()
            .generation,
        1
    );
}

#[tokio::test]
async fn subject_revision_overflow_is_rejected_instead_of_saturating() {
    let store = MemoryAuthStore::default();
    store
        .subjects
        .lock()
        .await
        .insert(SubjectId("subject-a".into()), subject_state(u64::MAX));

    assert!(matches!(
        store
            .replace_subject(u64::MAX, subject_state(u64::MAX))
            .await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn credential_generation_overflow_is_rejected_instead_of_saturating() {
    let store = MemoryAuthStore::default();
    store
        .credentials
        .lock()
        .await
        .insert(SessionId("sesn_a".into()), credential_state(u64::MAX));

    assert!(matches!(
        store
            .replace_credential(u64::MAX, credential_state(u64::MAX))
            .await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn store_failure_is_distinct_from_absent_state() {
    let store = MemoryAuthStore::default();
    store.set_unavailable(true);
    assert!(matches!(
        store
            .browser_session(&SecretDigest::identify("browser-session", "missing"))
            .await,
        Err(StoreError::Unavailable(_))
    ));
}
