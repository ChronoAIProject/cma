use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt as _;

use super::*;

fn cipher() -> AuthCipher {
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    AuthCipher::new("test-deployment", &key).unwrap()
}

fn subject_state() -> SubjectAuthState {
    SubjectAuthState {
        subject: SubjectId("subject-a".into()),
        revision: 7,
        tokens: Some(DurableSubjectTokens {
            access_token: SecretValue::new("access-plaintext".into()),
            refresh_token: Some(SecretValue::new("refresh-plaintext".into())),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000),
        }),
        refresh_lease: Some(SubjectRefreshLease {
            holder: "refresh-worker-a".into(),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060),
        }),
        authorization: Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::ReadAllSessions,
        ]),
        profile: Profile {
            display_name: Some("Example Person".into()),
            email: Some("person@example.test".into()),
            avatar_url: None,
        },
    }
}

fn credential_state() -> SessionCredentialState {
    SessionCredentialState {
        session: SessionId("sesn_a".into()),
        generation: 9,
        current: Some(DurableCredential {
            id: "provider-current".into(),
            value: SecretValue::new("current-plaintext".into()),
            owner: SubjectId("subject-a".into()),
            issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            expires_at: None,
            scope_fingerprint: "scope-a".into(),
        }),
        pending: Some(DurableCredential {
            id: "provider-pending".into(),
            value: SecretValue::new("pending-plaintext".into()),
            owner: SubjectId("subject-a".into()),
            issued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_001),
            expires_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_001)),
            scope_fingerprint: "scope-b".into(),
        }),
        revoke_debt: vec![RevokeDebt {
            id: "provider-retired".into(),
            owner: SubjectId("subject-a".into()),
            queued_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_002),
        }],
        lease: Some(CredentialLease {
            holder: "worker-a".into(),
            fence: 9,
            acquired_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_003),
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_063),
            scope_fingerprint: "scope-b".into(),
        }),
        retired: false,
    }
}

#[test]
fn expiry_indexes_are_zero_second_ttl_cleanup_indexes() {
    for name in [BROWSER_EXPIRY_INDEX, LOGIN_EXPIRY_INDEX] {
        let index = expiry_index(name);
        assert_eq!(index.keys, doc! { "expires_at": 1 });
        let options = index.options.unwrap();
        assert_eq!(options.name.as_deref(), Some(name));
        assert_eq!(options.expire_after, Some(Duration::ZERO));
    }
}

#[test]
fn subject_secrets_are_encrypted_and_round_trip() {
    let cipher = cipher();
    let stored = subject_to_doc(subject_state(), &cipher).unwrap();
    let rendered = format!(
        "{:?}",
        mongodb::bson::serialize_to_document(&stored).unwrap()
    );
    assert!(!rendered.contains("access-plaintext"));
    assert!(!rendered.contains("refresh-plaintext"));

    let restored = subject_from_doc(stored, &cipher).unwrap();
    let tokens = restored.tokens.unwrap();
    assert_eq!(tokens.access_token.expose(), "access-plaintext");
    assert_eq!(
        tokens.refresh_token.as_ref().unwrap().expose(),
        "refresh-plaintext"
    );
    assert_eq!(restored.revision, 7);
    let refresh_lease = restored.refresh_lease.unwrap();
    assert_eq!(refresh_lease.holder, "refresh-worker-a");
    assert_eq!(
        refresh_lease.expires_at,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_060)
    );
    assert!(restored.authorization.allows(Capability::ReadAllSessions));
    assert_eq!(
        restored.profile.display_name.as_deref(),
        Some("Example Person")
    );
}

#[test]
fn moving_subject_ciphertext_to_another_field_fails_closed() {
    let cipher = cipher();
    let mut stored = subject_to_doc(subject_state(), &cipher).unwrap();
    let tokens = stored.tokens.as_mut().unwrap();
    std::mem::swap(
        &mut tokens.access_token,
        tokens.refresh_token.as_mut().unwrap(),
    );
    assert!(matches!(
        subject_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn credential_values_are_encrypted_and_bound_to_current_or_pending() {
    let cipher = cipher();
    let mut stored = credential_state_to_doc(credential_state(), &cipher).unwrap();
    let rendered = format!(
        "{:?}",
        mongodb::bson::serialize_to_document(&stored).unwrap()
    );
    assert!(!rendered.contains("current-plaintext"));
    assert!(!rendered.contains("pending-plaintext"));

    let current = stored.current.as_mut().unwrap();
    let pending = stored.pending.as_mut().unwrap();
    std::mem::swap(&mut current.value, &mut pending.value);
    assert!(matches!(
        credential_state_from_doc(stored, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn credential_state_round_trips_every_durable_transition_fact() {
    let cipher = cipher();
    let restored = credential_state_from_doc(
        credential_state_to_doc(credential_state(), &cipher).unwrap(),
        &cipher,
    )
    .unwrap();
    assert_eq!(restored.session.as_str(), "sesn_a");
    assert_eq!(restored.generation, 9);
    assert_eq!(
        restored.current.unwrap().value.expose(),
        "current-plaintext"
    );
    assert_eq!(
        restored.pending.unwrap().value.expose(),
        "pending-plaintext"
    );
    assert_eq!(restored.revoke_debt[0].id, "provider-retired");
    assert_eq!(restored.lease.unwrap().fence, 9);
    assert!(!restored.retired);
}

#[test]
fn negative_stored_counters_and_unknown_capabilities_fail_closed() {
    let cipher = cipher();
    let mut subject = subject_to_doc(subject_state(), &cipher).unwrap();
    subject.revision = -1;
    assert!(matches!(
        subject_from_doc(subject, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut subject = subject_to_doc(subject_state(), &cipher).unwrap();
    subject.capabilities.push("future-superuser".into());
    assert!(matches!(
        subject_from_doc(subject, &cipher),
        Err(StoreError::Malformed(_))
    ));

    let mut credentials = credential_state_to_doc(credential_state(), &cipher).unwrap();
    credentials.generation = -1;
    assert!(matches!(
        credential_state_from_doc(credentials, &cipher),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn every_capability_round_trips_through_subject_storage() {
    for capability in Capability::ALL {
        assert_eq!(
            super::capability(capability_name(capability)).unwrap(),
            capability
        );
    }
}

#[test]
fn revisions_must_advance_exactly_once_without_overflow() {
    assert!(validate_next(4, 5, "record").is_ok());
    assert!(validate_next(4, 4, "record").is_err());
    assert!(validate_next(4, 6, "record").is_err());
    assert!(validate_next(u64::MAX, u64::MAX, "record").is_err());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn real_mongo_has_exact_indexes_and_cross_instance_atomicity() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
    let client = Client::with_uri_str(&uri).await.unwrap();
    let database_name = format!("chrono_auth_test_{}", uuid::Uuid::new_v4().simple());
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    let first = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    let second = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();

    for (collection, expected_name) in [
        (BROWSER_SESSIONS, BROWSER_EXPIRY_INDEX),
        (LOGIN_ATTEMPTS, LOGIN_EXPIRY_INDEX),
    ] {
        let indexes = client
            .database(&database_name)
            .collection::<mongodb::bson::Document>(collection)
            .list_indexes()
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let expiry = indexes
            .into_iter()
            .find(|index| {
                index
                    .options
                    .as_ref()
                    .and_then(|options| options.name.as_deref())
                    == Some(expected_name)
            })
            .expect("named expiry index exists");
        assert_eq!(expiry.keys, doc! { "expires_at": 1 });
        assert_eq!(expiry.options.unwrap().expire_after, Some(Duration::ZERO));
    }

    let browser_id = SecretDigest::identify("browser-session", "browser-cookie");
    first
        .put_browser_session(BrowserSession {
            id: browser_id.clone(),
            subject: SubjectId("subject-a".into()),
            expires_at: SystemTime::now() + Duration::from_secs(600),
        })
        .await
        .unwrap();
    assert_eq!(
        second
            .browser_session(&browser_id)
            .await
            .unwrap()
            .unwrap()
            .subject
            .as_str(),
        "subject-a"
    );

    let login_id = SecretDigest::identify("login-attempt", "oauth-state");
    first
        .put_login_attempt(LoginAttempt {
            id: login_id.clone(),
            verifier: SecretValue::new("pkce-plaintext".into()),
            expires_at: SystemTime::now() + Duration::from_secs(300),
        })
        .await
        .unwrap();
    let raw_login = client
        .database(&database_name)
        .collection::<mongodb::bson::Document>(LOGIN_ATTEMPTS)
        .find_one(doc! { "_id": id_binary(&login_id) })
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{raw_login:?}").contains("pkce-plaintext"));

    let (left, right) = tokio::join!(
        first.take_login_attempt(&login_id),
        second.take_login_attempt(&login_id)
    );
    let consumed = [left.unwrap(), right.unwrap()];
    assert_eq!(consumed.iter().filter(|value| value.is_some()).count(), 1);
    assert_eq!(
        consumed
            .into_iter()
            .flatten()
            .next()
            .unwrap()
            .verifier
            .expose(),
        "pkce-plaintext"
    );

    let mut initial = subject_state();
    initial.revision = 0;
    assert!(first.insert_subject(initial).await.unwrap());
    let mut changed_by_first = subject_state();
    changed_by_first.revision = 1;
    changed_by_first.profile.display_name = Some("first".into());
    let mut changed_by_second = subject_state();
    changed_by_second.revision = 1;
    changed_by_second.profile.display_name = Some("second".into());
    let (left, right) = tokio::join!(
        first.replace_subject(0, changed_by_first),
        second.replace_subject(0, changed_by_second)
    );
    assert_eq!(
        [left.unwrap(), right.unwrap()]
            .into_iter()
            .filter(|won| *won)
            .count(),
        1
    );

    drop(first);
    drop(second);
    let restarted = MongoAuthStore::open(&client, &database_name, "deployment-a", &key)
        .await
        .unwrap();
    assert!(restarted
        .browser_session(&browser_id)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        restarted
            .subject(&SubjectId("subject-a".into()))
            .await
            .unwrap()
            .unwrap()
            .revision,
        1
    );

    client.database(&database_name).drop().await.unwrap();
}
