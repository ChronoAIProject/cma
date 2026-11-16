use super::*;
use crate::auth::{persistence::*, profile::Profile, relay::tests::Fixture};
use axum::http::{Method, Uri};
use layer2_core::{Authorization, SubjectId};

#[tokio::test]
async fn receiver_reconstructs_exact_command_custody_and_rejects_forged_actor() {
    let f = Fixture::new(true).await;
    let identity = f
        .auth
        .capture_owner_admission(&f.context, "test-access")
        .await
        .unwrap();
    let restored = f
        .auth
        .restore_owner_admission(&identity, "test-access")
        .await
        .unwrap();
    assert_eq!(restored.caller, f.context.caller);
    assert_eq!(restored.actor, f.context.actor);
    assert_eq!(restored.credential, f.context.credential);
    assert_eq!(restored.credential_source, f.context.credential_source);
    let mut forged = identity.clone();
    forged.actor = AgentActor::Api {
        subject: SubjectId::new("another-user"),
        delegation_id: None,
    };
    assert!(matches!(
        f.auth.restore_owner_admission(&forged, "test-access").await,
        Err(CommandAuthorizationError::Revoked)
    ));
    assert!(matches!(
        f.auth
            .restore_owner_admission(&identity, "changed-credential")
            .await,
        Err(CommandAuthorizationError::Revoked)
    ));
}

#[tokio::test]
async fn same_subject_and_token_in_a_new_signin_cannot_inherit_an_inflight_admission() {
    let f = Fixture::new(true).await;
    let identity = f
        .auth
        .capture_owner_admission(&f.context, "test-access")
        .await
        .unwrap();
    f.store
        .put_subject(SubjectAuthState::new(
            SubjectId::new("owner"),
            Authorization::none(),
            Profile::default(),
            Some(DurableSubjectTokens {
                access_token: SecretValue::new("test-access".into()),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        ))
        .await
        .unwrap();
    assert!(matches!(
        f.auth
            .restore_owner_admission(&identity, "test-access")
            .await,
        Err(CommandAuthorizationError::Revoked)
    ));
}

#[tokio::test]
async fn receiver_does_not_exchange_a_logged_out_browser_for_retained_token_custody() {
    let f = Fixture::new(true).await;
    let identity = f
        .auth
        .capture_owner_admission(&f.context, "test-access")
        .await
        .unwrap();
    let restored = f
        .auth
        .restore_owner_admission(&identity, "test-access")
        .await
        .unwrap();
    let uri: Uri = "/api/v2/responses".parse().unwrap();
    f.auth
        .verify_owner_admission_origin(&restored, "test-access", &Method::POST, &uri, &f.headers)
        .await
        .unwrap();
    f.store
        .delete_browser_login(&SecretDigest::identify(
            crate::auth::AUTH_COOKIE_DOMAIN,
            "test-login",
        ))
        .await
        .unwrap();
    assert!(f
        .auth
        .verify_owner_admission_origin(&restored, "test-access", &Method::POST, &uri, &f.headers)
        .await
        .is_err());
}

#[tokio::test]
async fn independent_bearer_custody_cannot_substitute_retained_browser_credentials() {
    let f = Fixture::independent().await;
    let identity = f
        .auth
        .capture_owner_admission(&f.context, &f.credential)
        .await
        .unwrap();
    let restored = f
        .auth
        .restore_owner_admission(&identity, &f.credential)
        .await
        .unwrap();
    assert_eq!(restored.credential_source, f.context.credential_source);
    let uri: Uri = "/api/v2/responses".parse().unwrap();
    f.auth
        .verify_owner_admission_origin(&restored, &f.credential, &Method::POST, &uri, &f.headers)
        .await
        .unwrap();
    let mut changed = f.headers.clone();
    changed.insert(
        "authorization",
        "Bearer another-credential".parse().unwrap(),
    );
    assert!(f
        .auth
        .verify_owner_admission_origin(&restored, &f.credential, &Method::POST, &uri, &changed)
        .await
        .is_err());
}

#[tokio::test]
async fn request_bound_read_proof_cannot_capture_mutation_custody() {
    let f = Fixture::new(true).await;
    let mut read = f.context.clone();
    read.credential_source = CredentialSource::RequestBoundRead {
        expires_at: SystemTime::now() + Duration::from_secs(30),
    };
    assert!(matches!(
        f.auth.capture_owner_admission(&read, "test-access").await,
        Err(CommandAuthorizationError::Revoked)
    ));
}

mod binding_tests;
