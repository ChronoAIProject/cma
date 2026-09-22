use super::*;
use layer2_core::bridge::{BridgeError, BridgeSetupBlock};
use layer2_core::resource_access::{CallerContext, ResourceActor, ResourceOwner};

fn owner() -> ResourceOwner {
    ResourceOwner::Person {
        subject: SubjectId::new("owner"),
    }
}
fn org() -> ResourceOwner {
    ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    }
}
fn context(f: &Fixture, state: &SubjectAuthState) -> CallerContext {
    CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: layer2_core::Caller::new(state.subject.clone(), state.authorization.clone()),
        credential: f
            .auth
            .provider_credential_context(
                &state.subject,
                state.tokens.as_ref().unwrap().access_token.expose(),
            )
            .unwrap(),
        actor: ResourceActor::Human {
            subject: state.subject.clone(),
        },
        memberships: vec![],
    }
}

#[tokio::test]
async fn grant_capture_requires_exact_current_renewable_human_custody() {
    let f = Fixture::new().await;
    let mut state = f.seed().await;
    let context = context(&f, &state);
    for credential in ["different-human-login", "nyxid_ag_selected-key", ""] {
        assert!(f
            .auth
            .capture_bridge_human(&context, credential, &owner())
            .await
            .is_err());
    }
    assert_eq!(f.control.requests.load(Ordering::SeqCst), 0);
    let lease = f
        .auth
        .capture_bridge_human(&context, "must-not-leak-access-token", &owner())
        .await
        .unwrap();
    assert_eq!(lease.subject(), &state.subject);
    assert_eq!(lease.incarnation(), state.incarnation);
    assert_eq!(lease.actor_context().caller.subject, state.subject);
    state.tokens.as_mut().unwrap().refresh_token = None;
    f.store.put_subject(state).await.unwrap();
    assert!(matches!(
        f.auth
            .capture_bridge_human(&context, "must-not-leak-access-token", &owner())
            .await,
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::RenewableAuthorityUnavailable
        ))
    ));
}

#[tokio::test]
async fn deliberate_org_authority_survives_cookie_logout_and_refreshes_exact_sign_in() {
    let f = Fixture::new().await;
    let mut state = f.seed().await;
    let admitted = f
        .auth
        .capture_bridge_human(&context(&f, &state), "must-not-leak-access-token", &org())
        .await
        .unwrap();
    let login = f
        .auth
        .issue_browser_login(state.subject.clone())
        .await
        .unwrap();
    f.auth.revoke_browser_login(&login).await.unwrap();
    state.tokens.as_mut().unwrap().expires_at = SystemTime::now() + Duration::from_secs(10);
    f.store.put_subject(state.clone()).await.unwrap();
    *f.control.organization_token.write().await = "rotated-fixture-token".into();
    let renewed = f
        .auth
        .resolve_bridge_human(admitted.subject(), &org())
        .await
        .unwrap();
    assert_eq!(renewed.subject(), &SubjectId::new("owner"));
    assert_eq!(renewed.incarnation(), admitted.incarnation());
    assert_eq!(renewed.credential(), "rotated-fixture-token");
    assert_eq!(
        renewed.actor_context().memberships[0].organization,
        SubjectId::new("org")
    );
    assert_eq!(
        renewed.actor_context().caller.subject,
        SubjectId::new("owner")
    );
    let current = f.store.subject(&state.subject).await.unwrap().unwrap();
    assert_eq!(current.incarnation, state.incarnation);
    assert_eq!(
        current.tokens.unwrap().refresh_token.unwrap().expose(),
        "rotated-fixture-refresh"
    );
    assert!(f.auth.revalidate_bridge_human(&admitted).await.is_err());
}

#[tokio::test]
async fn retained_bridge_human_uses_fresh_same_subject_sign_in_and_fences_old_lease() {
    for resource_owner in [owner(), org()] {
        let f = Fixture::new().await;
        let state = f.seed().await;
        let mut prior = f
            .auth
            .capture_bridge_human(
                &context(&f, &state),
                "must-not-leak-access-token",
                &resource_owner,
            )
            .await
            .unwrap();
        for token in ["fresh-login-one", "fresh-login-two"] {
            // Use the actual successful OAuth login persistence path, which
            // intentionally replaces incarnation for deferred-command fencing.
            f.auth
                .store_sign_in(
                    state.subject.clone(),
                    DurableSubjectTokens {
                        access_token: SecretValue::new(token.into()),
                        refresh_token: Some(SecretValue::new("fresh-refresh".into())),
                        expires_at: SystemTime::now() + Duration::from_secs(3600),
                    },
                    state.authorization.clone(),
                    state.profile.clone(),
                )
                .await
                .unwrap();
            let current = f.store.subject(&state.subject).await.unwrap().unwrap();
            assert_ne!(current.incarnation, prior.incarnation());
            assert!(f.auth.revalidate_bridge_human(&prior).await.is_err());
            *f.control.organization_token.write().await = token.into();
            let renewed = f
                .auth
                .resolve_bridge_human(&state.subject, &resource_owner)
                .await
                .unwrap();
            assert_eq!(renewed.subject(), &state.subject);
            assert_eq!(renewed.incarnation(), current.incarnation);
            assert_eq!(renewed.credential(), token);
            f.auth.revalidate_bridge_human(&renewed).await.unwrap();
            prior = renewed;
        }
        if resource_owner == org() {
            assert_eq!(f.control.organization_requests.load(Ordering::SeqCst), 3);
        }
    }
}

#[tokio::test]
async fn fresh_sign_in_does_not_allow_bridge_to_borrow_another_humans_custody() {
    let f = Fixture::new().await;
    let state = f.seed().await;
    f.auth
        .store_sign_in(
            SubjectId::new("another-human"),
            state.tokens.clone().unwrap(),
            state.authorization.clone(),
            state.profile.clone(),
        )
        .await
        .unwrap();
    assert!(f
        .auth
        .resolve_bridge_human(&SubjectId::new("another-human"), &owner())
        .await
        .is_err());
    let mut removed = state.clone();
    removed.tokens = None;
    f.store.put_subject(removed).await.unwrap();
    assert!(f
        .auth
        .resolve_bridge_human(&state.subject, &owner())
        .await
        .is_err());
}

#[tokio::test]
async fn retained_authority_rechecks_exact_org_role_account_and_sign_in() {
    let f = Fixture::new().await;
    let state = f.seed().await;
    for role in ["member", "viewer"] {
        *f.control.organization.write().await = Reply::json(json!({"id":"org","your_role":role}));
        assert!(f
            .auth
            .resolve_bridge_human(&state.subject, &org())
            .await
            .is_err());
    }
    *f.control.organization.write().await =
        Reply::json(json!({"id":"other-org","your_role":"admin"}));
    assert!(f
        .auth
        .resolve_bridge_human(&state.subject, &org())
        .await
        .is_err());
    *f.control.organization.write().await = Reply::json(json!({"id":"org","your_role":"admin"}));
    let lease = f
        .auth
        .resolve_bridge_human(&state.subject, &org())
        .await
        .unwrap();
    *f.control.introspection.write().await = Reply::json(json!({"active":false}));
    assert!(f
        .auth
        .resolve_bridge_human(&state.subject, &org())
        .await
        .is_err());
    let replacement = SubjectAuthState::new(
        state.subject.clone(),
        state.authorization.clone(),
        state.profile.clone(),
        state.tokens.clone(),
    );
    f.store.put_subject(replacement).await.unwrap();
    assert!(f.auth.revalidate_bridge_human(&lease).await.is_err());
    assert!(f
        .auth
        .resolve_bridge_human(&state.subject, &owner())
        .await
        .is_err());
}

#[tokio::test]
async fn sign_in_replacement_during_org_io_fences_buffered_success() {
    let f = Fixture::new().await;
    let state = f.seed().await;
    let gate = Arc::new(Gate::default());
    f.control.organization.write().await.gate = Some(gate.clone());
    let auth = f.auth.clone();
    let subject = state.subject.clone();
    let pending = tokio::spawn(async move { auth.resolve_bridge_human(&subject, &org()).await });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    f.auth
        .store_sign_in(
            state.subject,
            state.tokens.unwrap(),
            state.authorization,
            state.profile,
        )
        .await
        .unwrap();
    gate.release.notify_one();
    assert!(pending.await.unwrap().is_err());
}

#[tokio::test]
async fn removed_tokens_and_delegated_or_raw_key_custody_never_refresh_as_humans() {
    use base64::Engine as _;
    let f = Fixture::new().await;
    let state = f.seed().await;
    let delegated = format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"delegated":true}"#)
    );
    for token in [None, Some("nyxid_ag_a"), Some(delegated.as_str())] {
        let mut changed = state.clone();
        if let Some(token) = token {
            changed.tokens.as_mut().unwrap().access_token = SecretValue::new(token.into());
        } else {
            changed.tokens = None;
        }
        f.store.put_subject(changed).await.unwrap();
        assert!(f
            .auth
            .resolve_bridge_human(&state.subject, &org())
            .await
            .is_err());
    }
    assert_eq!(f.control.requests.load(Ordering::SeqCst), 0);
}
