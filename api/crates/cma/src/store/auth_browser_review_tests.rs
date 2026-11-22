use super::*;
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};

// Exact predecessor shapes from develop@09823a2ff6a0, intentionally without
// deny_unknown_fields. Active sessions must remain readable in either direction.
#[derive(Serialize, Deserialize)]
struct PredecessorBrowserLoginDoc {
    #[serde(rename = "_id")]
    id: Binary,
    document_schema_version: i64,
    subject: String,
    expires_at: DateTime,
}
#[derive(Serialize, Deserialize)]
struct PredecessorLoginAttemptDoc {
    #[serde(rename = "_id")]
    id: Binary,
    document_schema_version: i64,
    browser_binding: Binary,
    verifier: CipherEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    return_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor_link_intent: Option<CipherEnvelope>,
    expires_at: DateTime,
}
fn login(value: &str) -> BrowserLogin {
    BrowserLogin {
        id: SecretDigest::identify("browser-login", value),
        subject: SubjectId::new("owner"),
        expires_at: SystemTime::now() + Duration::from_secs(3600),
    }
}
fn attempt() -> LoginAttempt {
    LoginAttempt {
        id: SecretDigest::identify("attempt", "state"),
        browser_binding: SecretDigest::identify("binding", "binding"),
        verifier: SecretValue::new("fixture-verifier".into()),
        return_to: Some("/agent-manager".into()),
        actor_link_intent: None,
        service_authorization: None,
        silent: false,
        consent_requested: false,
        custody_reserved: true,
        started_at: Some(SystemTime::now()),
        expires_at: SystemTime::now() + Duration::from_secs(600),
    }
}

#[tokio::test]
async fn predecessor_attempt_round_trips_ignore_new_fields_and_default_to_legacy_issuance() {
    let store = super::tests::store().await;
    let new = store.login_to_doc(attempt()).unwrap();
    let bson = mongodb::bson::serialize_to_document(&new).unwrap();
    assert!(bson.get_bool("custody_reserved").unwrap());
    let old: PredecessorLoginAttemptDoc = mongodb::bson::deserialize_from_document(bson).unwrap();
    let old_bson = mongodb::bson::serialize_to_document(&old).unwrap();
    assert!(!old_bson.contains_key("custody_reserved"));
    assert!(!old_bson.contains_key("started_at"));
    let restored = store
        .login_from_doc(mongodb::bson::deserialize_from_document(old_bson).unwrap())
        .unwrap();
    assert!(!restored.custody_reserved);
    assert!(restored.started_at.is_none());
    let again =
        mongodb::bson::serialize_to_document(&store.login_to_doc(restored).unwrap()).unwrap();
    assert!(
        !again.contains_key("custody_reserved"),
        "false must be omitted for the predecessor"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_fresh_session_and_link_are_atomic_and_predecessor_readable() {
    let (client, name, first, second) = super::refresh_tests::stores().await;
    let binding = SecretDigest::identify("browser-login", "planted-binding");
    let login = login("fresh-session");
    assert!(first
        .reserve_browser_login(
            binding.clone(),
            SystemTime::now() + Duration::from_secs(600)
        )
        .await
        .unwrap());
    let mut tx = first.client.start_session().await.unwrap();
    start_transaction(&mut tx).await.unwrap();
    assert!(first
        .link_browser_reservation(&binding, &login, None, &mut tx)
        .await
        .unwrap());
    assert!(second.browser_login(&login.id).await.unwrap().is_none());
    assert!(second.browser_login(&binding).await.unwrap().is_none());
    first
        .insert_linked_browser_login(&binding, &login, &mut tx)
        .await
        .unwrap();
    assert!(
        second.browser_login(&login.id).await.unwrap().is_none(),
        "uncommitted session must not escape"
    );
    assert!(matches!(
        commit_transaction(&mut tx).await,
        CommitOutcome::Committed
    ));
    let row = second
        .browser_logins
        .find_one(doc! {"_id":binary(*login.id.as_bytes())})
        .await
        .unwrap()
        .unwrap();
    let old: PredecessorBrowserLoginDoc = mongodb::bson::deserialize_from_document(
        mongodb::bson::serialize_to_document(&row).unwrap(),
    )
    .unwrap();
    assert_eq!(old.subject, "owner");
    assert!(row.binding_id.is_some());
    let link = second
        .browser_logins
        .find_one(doc! {"_id":binary(*binding.as_bytes())})
        .await
        .unwrap()
        .unwrap();
    assert!(
        link.expires_at >= row.expires_at,
        "link must outlive any delayed response"
    );
    assert!(
        second.browser_login(&binding).await.unwrap().is_none(),
        "known pre-auth binding is never identity"
    );
    second.revoke_browser_binding(&binding).await.unwrap();
    assert!(first.browser_login(&login.id).await.unwrap().is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_logout_before_between_and_after_promotion_never_leaves_a_session() {
    let (client, name, first, second) = super::refresh_tests::stores().await;
    for step in [
        "before-adoption",
        "before-link",
        "between-link-and-insert",
        "after-insert-before-commit",
        "after-commit-binding",
        "after-commit-session",
    ] {
        let binding = SecretDigest::identify("browser-login", step);
        let login = login(&format!("fresh-{step}"));
        if step == "before-adoption" {
            let revoked_at = SystemTime::now();
            second.revoke_browser_binding(&binding).await.unwrap();
            let tombstone = first
                .browser_logins
                .find_one(doc! {"_id":binary(*binding.as_bytes())})
                .await
                .unwrap()
                .unwrap();
            assert!(
                tombstone.expires_at
                    >= DateTime::from_system_time(
                        revoked_at + crate::auth::BROWSER_BINDING_LIFETIME
                    )
            );
            assert!(!first
                .reserve_browser_login(
                    binding.clone(),
                    SystemTime::now() + Duration::from_secs(600)
                )
                .await
                .unwrap());
        } else {
            assert!(first
                .reserve_browser_login(
                    binding.clone(),
                    SystemTime::now() + Duration::from_secs(600)
                )
                .await
                .unwrap());
            match step {
                "before-link" => {
                    second.revoke_browser_binding(&binding).await.unwrap();
                    assert!(!first
                        .complete_browser_login(&binding, login.clone(), None)
                        .await
                        .unwrap());
                }
                "between-link-and-insert" | "after-insert-before-commit" => {
                    let mut tx = first.client.start_session().await.unwrap();
                    start_transaction(&mut tx).await.unwrap();
                    assert!(first
                        .link_browser_reservation(&binding, &login, None, &mut tx)
                        .await
                        .unwrap());
                    if step == "after-insert-before-commit" {
                        first
                            .insert_linked_browser_login(&binding, &login, &mut tx)
                            .await
                            .unwrap();
                    }
                    // A competing logout cannot claim success over uncommitted writes.
                    assert!(tokio::time::timeout(
                        Duration::from_secs(5),
                        second.revoke_browser_binding(&binding)
                    )
                    .await
                    .unwrap()
                    .is_err());
                    tx.abort_transaction().await.unwrap();
                    second.revoke_browser_binding(&binding).await.unwrap();
                    assert!(!first
                        .complete_browser_login(&binding, login.clone(), None)
                        .await
                        .unwrap());
                }
                _ => {
                    assert!(first
                        .complete_browser_login(&binding, login.clone(), None)
                        .await
                        .unwrap());
                    if step == "after-commit-session" {
                        second.delete_browser_login(&login.id).await.unwrap();
                    } else {
                        second.revoke_browser_binding(&binding).await.unwrap();
                    }
                }
            }
        }
        assert!(
            first.browser_login(&login.id).await.unwrap().is_none(),
            "{step}"
        );
        assert!(
            !first
                .complete_browser_login(&binding, login, None)
                .await
                .unwrap(),
            "{step}"
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_predecessor_callback_ignores_new_reservation_and_ttl_still_covers_it() {
    let (client, name, first, second) = super::refresh_tests::stores().await;
    let attempt = attempt();
    let expiry = attempt.expires_at;
    let binding = SecretDigest::identify("browser-login", "old-reader-binding");
    first
        .reserve_browser_login(binding.clone(), expiry)
        .await
        .unwrap();
    let old: PredecessorLoginAttemptDoc = mongodb::bson::deserialize_from_document(
        mongodb::bson::serialize_to_document(&first.login_to_doc(attempt).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(old.return_to.as_deref(), Some("/agent-manager"));
    // The old callback consumes its understood fields and mints its own fresh row.
    let issued = login("old-reader-fresh-session");
    let predecessor = PredecessorBrowserLoginDoc {
        id: binary(*issued.id.as_bytes()),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        subject: "owner".into(),
        expires_at: DateTime::from_system_time(issued.expires_at),
    };
    first
        .browser_logins
        .clone_with_type::<PredecessorBrowserLoginDoc>()
        .insert_one(predecessor)
        .await
        .unwrap();
    assert_eq!(
        second
            .browser_login(&issued.id)
            .await
            .unwrap()
            .unwrap()
            .subject
            .as_str(),
        "owner"
    );
    let pending = first
        .browser_logins
        .find_one(doc! {"_id":binary(*binding.as_bytes())})
        .await
        .unwrap()
        .unwrap();
    assert!(pending.subject.is_none() && pending.linked_session.is_none());
    assert_eq!(pending.expires_at, DateTime::from_system_time(expiry));
    use futures_util::TryStreamExt;
    let indexes = first
        .browser_logins
        .list_indexes()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert!(indexes
        .iter()
        .any(|index| index.keys == doc! {"expires_at":1}
            && index.options.as_ref().and_then(|o| o.expire_after) == Some(Duration::ZERO)));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_preference_is_causal_and_separate_from_cross_site_session_revocation() {
    let (client, name, first, second) = super::refresh_tests::stores().await;
    let binding = SecretDigest::identify("browser-login", "preference-binding");
    let login = login("preference-fresh");
    let started = SystemTime::now() + Duration::from_secs(60);
    first
        .reserve_browser_login(
            binding.clone(),
            SystemTime::now() + Duration::from_secs(600),
        )
        .await
        .unwrap();
    assert!(first
        .complete_browser_login(&binding, login.clone(), Some(started))
        .await
        .unwrap());
    assert!(second
        .browser_sign_in_preference(&binding, started)
        .await
        .unwrap());
    assert!(!second
        .browser_sign_in_preference(&binding, started - Duration::from_secs(1))
        .await
        .unwrap());
    second
        .logout_browser_custody(&login.id, false, false)
        .await
        .unwrap();
    assert!(first.browser_login(&login.id).await.unwrap().is_none());
    assert!(
        first
            .browser_sign_in_preference(&binding, started)
            .await
            .unwrap(),
        "cross-site logout cannot reactivate an old opt-out"
    );
    second.revoke_browser_binding(&binding).await.unwrap();
    assert!(
        !first
            .browser_sign_in_preference(&binding, started)
            .await
            .unwrap(),
        "explicit logout wins even over a fast start clock or stale cookie"
    );
    client.database(&name).drop().await.unwrap();
}
