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
            service_authorization: None,
            silent: false,
            consent_requested: false,
            custody_reserved: false,
            started_at: None,
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

#[tokio::test]
async fn fresh_sessions_remain_distinct_and_either_handle_revokes_them_in_memory() {
    for step in [
        "before-reserve",
        "before-complete",
        "after-binding",
        "after-session",
        "race",
    ] {
        let store = std::sync::Arc::new(MemoryAuthStore::default());
        let replica = std::sync::Arc::new(store.as_ref().clone());
        assert!(!std::sync::Arc::ptr_eq(&store, &replica));
        let binding = SecretDigest::identify("browser-login", step);
        let login = BrowserLogin {
            id: SecretDigest::identify("browser-login", &format!("fresh-{step}")),
            subject: SubjectId::new("owner"),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        };
        if step == "before-reserve" {
            replica.revoke_browser_binding(&binding).await.unwrap();
            assert!(!store
                .reserve_browser_login(binding.clone(), SystemTime::now() + Duration::from_secs(60))
                .await
                .unwrap());
        } else {
            assert!(store
                .reserve_browser_login(binding.clone(), SystemTime::now() + Duration::from_secs(60))
                .await
                .unwrap());
            match step {
                "before-complete" => {
                    replica.revoke_browser_binding(&binding).await.unwrap();
                    assert!(!store
                        .complete_browser_login(&binding, login.clone(), None)
                        .await
                        .unwrap());
                }
                "race" => {
                    let (done, out) = tokio::join!(
                        store.complete_browser_login(&binding, login.clone(), None),
                        replica.revoke_browser_binding(&binding)
                    );
                    done.unwrap();
                    out.unwrap();
                }
                _ => {
                    assert!(store
                        .complete_browser_login(&binding, login.clone(), None)
                        .await
                        .unwrap());
                    assert!(replica.browser_login(&binding).await.unwrap().is_none());
                    assert_eq!(
                        replica
                            .browser_login(&login.id)
                            .await
                            .unwrap()
                            .unwrap()
                            .subject
                            .as_str(),
                        "owner"
                    );
                    if step == "after-session" {
                        replica.delete_browser_login(&login.id).await.unwrap();
                    } else {
                        replica.revoke_browser_binding(&binding).await.unwrap();
                    }
                }
            }
        }
        assert!(store.browser_login(&login.id).await.unwrap().is_none());
        assert!(!store
            .complete_browser_login(&binding, login, None)
            .await
            .unwrap());
        assert!(!store
            .renew_browser_login(&binding, SystemTime::now() + Duration::from_secs(600))
            .await
            .unwrap());
    }
}
