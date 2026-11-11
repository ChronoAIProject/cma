use std::time::Duration;

use super::*;

#[test]
fn secrets_are_one_way_and_redacted() {
    let first = SecretDigest::identify("cookie", "value");
    let second = SecretDigest::identify("other", "value");
    assert_ne!(first, second);
    assert!(!format!("{first:?}").contains("value"));
    assert!(!format!("{:?}", SecretValue::new("visible".into())).contains("visible"));
}

#[tokio::test]
async fn login_attempts_are_binding_checked_and_single_use() {
    let store = MemoryAuthStore::default();
    let id = SecretDigest::identify("attempt", "state");
    let binding = SecretDigest::identify("binding", "browser");
    store
        .put_login_attempt(LoginAttempt {
            id: id.clone(),
            browser_binding: binding.clone(),
            verifier: SecretValue::new("verifier".into()),
            return_to: None,
            actor_link_intent: None,
            silent: false,
            consent_requested: false,
            expires_at: SystemTime::now() + Duration::from_secs(60),
        })
        .await
        .unwrap();
    assert!(store
        .take_login_attempt(&id, &SecretDigest::identify("binding", "other"))
        .await
        .unwrap()
        .is_none());
    assert!(store
        .take_login_attempt(&id, &binding)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .take_login_attempt(&id, &binding)
        .await
        .unwrap()
        .is_none());
}
