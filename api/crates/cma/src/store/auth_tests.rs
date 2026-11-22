use std::time::SystemTime;

use layer2_core::Capability;
use mongodb::bson::{spec::BinarySubtype, Binary};

use super::*;

const ACCESS_SECRET: &str = "access-secret";
const REFRESH_SECRET: &str = "refresh-secret";

pub(super) async fn store() -> MongoAuthStore {
    let client = Client::with_uri_str("mongodb://127.0.0.1:27017")
        .await
        .unwrap();
    let key =
        AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
    MongoAuthStore {
        client: client.clone(),
        browser_logins: client.database("test").collection(BROWSER_LOGINS),
        login_attempts: client.database("test").collection(LOGIN_ATTEMPTS),
        subjects: client.database("test").collection(SUBJECTS),
        api_bearers: client.database("test").collection(API_BEARERS),
        actor_links: client.database("test").collection(ACTOR_LINKS),
        proxy_grants: client.database("test").collection(PROXY_GRANTS),
        cipher: AuthCipher::new("test", &key).unwrap(),
    }
}

fn subject_state(tokens: Option<DurableSubjectTokens>) -> SubjectAuthState {
    SubjectAuthState::new(
        SubjectId::new("subject-a"),
        Authorization::new([Capability::UseOwnSandboxes, Capability::ReadActivityLog]),
        Profile {
            display_name: Some("Example Person".into()),
            email: Some("person@example.test".into()),
            avatar_url: Some("https://example.test/avatar.png".into()),
        },
        tokens,
    )
}

fn subject_tokens() -> DurableSubjectTokens {
    DurableSubjectTokens {
        access_token: SecretValue::new(ACCESS_SECRET.into()),
        refresh_token: Some(SecretValue::new(REFRESH_SECRET.into())),
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(10),
    }
}

#[tokio::test]
async fn login_verifier_is_encrypted_and_bound_to_its_attempt() {
    let store = store().await;
    let original = LoginAttempt {
        id: SecretDigest::identify("attempt", "state-a"),
        browser_binding: SecretDigest::identify("browser", "binding-a"),
        verifier: SecretValue::new("pkce-verifier-secret".into()),
        return_to: Some("/t/pub_test".into()),
        actor_link_intent: Some(crate::auth::actor_links::ActorLoginIntent {
            link_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            actor_key: "attested-actor-key".into(),
            revision: 3,
        }),
        service_authorization: None,
        silent: true,
        consent_requested: true,
        custody_reserved: false,
        started_at: None,
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
    };

    let document = store.login_to_doc(original.clone()).unwrap();
    assert_eq!(document.document_schema_version, DOCUMENT_SCHEMA_VERSION);
    assert!(!format!("{document:?}").contains(original.verifier.expose()));

    let restored = store.login_from_doc(document).unwrap();
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.browser_binding, original.browser_binding);
    assert_eq!(restored.verifier, original.verifier);
    assert_eq!(restored.return_to, original.return_to);
    assert_eq!(restored.actor_link_intent, original.actor_link_intent);
    assert_eq!(restored.silent, original.silent);
    assert_eq!(restored.consent_requested, original.consent_requested);
    assert_eq!(restored.expires_at, original.expires_at);

    let mut moved = store.login_to_doc(original).unwrap();
    moved.id = binary(*SecretDigest::identify("attempt", "state-b").as_bytes());
    assert!(matches!(
        store.login_from_doc(moved),
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn service_authorization_custody_keeps_the_bound_intent_encrypted() {
    use crate::auth::service_authorization::RecoveryIntent;
    use layer2_core::ServiceId;

    let store = store().await;
    let service = ServiceId::new("synthetic-service");
    let intent = RecoveryIntent {
        subject: SubjectId::new("synthetic-subject"),
        issuer: "https://issuing.example/".into(),
        required: [service.clone()].into(),
        requested: [service].into(),
        redirect_uri: "https://app.example/auth/service-authorization/callback".into(),
        return_to: "/agents/draft".into(),
    };
    let attempt = LoginAttempt {
        id: SecretDigest::identify("attempt", "consent"),
        browser_binding: SecretDigest::identify("browser", "original"),
        verifier: SecretValue::new("synthetic-verifier".into()),
        return_to: None,
        actor_link_intent: None,
        service_authorization: Some(intent.clone()),
        silent: false,
        consent_requested: false,
        custody_reserved: false,
        started_at: None,
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
    };
    let document = store.login_to_doc(attempt).unwrap();
    let view = format!("{document:?}");
    assert!(!view.contains(intent.subject.as_str()));
    assert!(!view.contains(&intent.return_to));
    assert_eq!(
        store
            .login_from_doc(document)
            .unwrap()
            .service_authorization,
        Some(intent)
    );
}

#[tokio::test]
async fn subject_secrets_and_metadata_round_trip() {
    let store = store().await;
    let original = subject_state(Some(subject_tokens()));
    let document = store.subject_to_doc(original.clone()).unwrap();
    assert_eq!(document.document_schema_version, DOCUMENT_SCHEMA_VERSION);
    let diagnostic = format!("{document:?}");
    assert!(!diagnostic.contains(ACCESS_SECRET));
    assert!(!diagnostic.contains(REFRESH_SECRET));

    let restored = store.subject_from_doc(document).unwrap();
    assert_eq!(restored.subject, original.subject);
    assert_eq!(restored.incarnation, original.incarnation);
    assert_eq!(restored.revision, original.revision);
    assert_eq!(restored.token_generation, original.token_generation);
    assert_eq!(restored.refresh_lease, original.refresh_lease);
    assert_eq!(restored.authorization, original.authorization);
    assert_eq!(restored.profile, original.profile);
    let restored_tokens = restored.tokens.unwrap();
    let original_tokens = original.tokens.unwrap();
    assert_eq!(restored_tokens.access_token, original_tokens.access_token);
    assert_eq!(restored_tokens.refresh_token, original_tokens.refresh_token);
    assert_eq!(restored_tokens.expires_at, original_tokens.expires_at);
}

#[test]
fn browser_login_documents_are_explicitly_schema_v1() {
    let document = MongoAuthStore::browser_login_to_doc(BrowserLogin {
        id: SecretDigest::identify("browser", "login"),
        subject: SubjectId::new("subject-a"),
        expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(20),
    });
    assert_eq!(document.document_schema_version, DOCUMENT_SCHEMA_VERSION);
}

#[tokio::test]
async fn tokenless_subject_round_trips_without_inventing_tokens() {
    let store = store().await;
    let original = subject_state(None);
    let restored = store
        .subject_from_doc(store.subject_to_doc(original.clone()).unwrap())
        .unwrap();

    assert_eq!(restored.subject, original.subject);
    assert_eq!(restored.authorization, original.authorization);
    assert_eq!(restored.profile, original.profile);
    assert!(restored.tokens.is_none());
}

#[tokio::test]
async fn partial_subject_token_shapes_fail_closed() {
    let store = store().await;

    let mut missing_expiry = store
        .subject_to_doc(subject_state(Some(subject_tokens())))
        .unwrap();
    missing_expiry.token_expires_at = None;
    assert!(matches!(
        store.subject_from_doc(missing_expiry),
        Err(StoreError::Malformed(_))
    ));

    let mut missing_access = store
        .subject_to_doc(subject_state(Some(subject_tokens())))
        .unwrap();
    missing_access.access_token = None;
    assert!(matches!(
        store.subject_from_doc(missing_access),
        Err(StoreError::Malformed(_))
    ));

    let mut encrypted = store
        .subject_to_doc(subject_state(Some(subject_tokens())))
        .unwrap();
    let refresh_token = encrypted.refresh_token.take();
    let mut refresh_only = store.subject_to_doc(subject_state(None)).unwrap();
    refresh_only.refresh_token = refresh_token;
    assert!(matches!(
        store.subject_from_doc(refresh_only),
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn moving_subject_ciphertext_between_fields_fails_integrity_validation() {
    let store = store().await;
    let mut document = store
        .subject_to_doc(subject_state(Some(subject_tokens())))
        .unwrap();
    std::mem::swap(&mut document.access_token, &mut document.refresh_token);

    assert!(matches!(
        store.subject_from_doc(document),
        Err(StoreError::Malformed(_))
    ));
}

#[test]
fn stored_digests_require_generic_binary_with_exactly_32_bytes() {
    let value = SecretDigest::identify("browser", "value");
    assert_eq!(
        digest(&binary(*value.as_bytes()), "browser").unwrap(),
        value
    );

    let unsupported = Binary {
        subtype: BinarySubtype::Uuid,
        bytes: vec![0; 32],
    };
    assert!(matches!(
        digest(&unsupported, "browser"),
        Err(StoreError::Malformed(_))
    ));

    let wrong_length = Binary {
        subtype: BinarySubtype::Generic,
        bytes: vec![0; 31],
    };
    assert!(matches!(
        digest(&wrong_length, "browser"),
        Err(StoreError::Malformed(_))
    ));
}
