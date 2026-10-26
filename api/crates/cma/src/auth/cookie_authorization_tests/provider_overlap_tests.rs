use super::*;

async fn overlapping_request(
    fixture: &Fixture,
    state: SubjectAuthState,
) -> (
    Arc<Gate>,
    Arc<Gate>,
    tokio::task::JoinHandle<Result<RequestAuth, StoreError>>,
) {
    let userinfo = Arc::new(Gate::default());
    let introspection = Arc::new(Gate::default());
    fixture.control.userinfo.write().await.gate = Some(userinfo.clone());
    fixture.control.introspection.write().await.gate = Some(introspection.clone());
    let auth = fixture.auth.clone();
    let pending =
        tokio::spawn(async move { request_for_cookie(&auth, &state.subject, state.clone()).await });
    // A serial implementation cannot reach both endpoints while neither reply
    // is released. This verifies overlap without a fragile elapsed-time limit.
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            userinfo.reached.notified(),
            introspection.reached.notified()
        );
    })
    .await
    .expect("userinfo and introspection must both start before either completes");
    (userinfo, introspection, pending)
}

#[tokio::test]
async fn both_fresh_provider_reads_overlap_but_neither_alone_grants_authority() {
    for userinfo_first in [true, false] {
        let fixture = Fixture::new().await;
        let state = fixture.seed().await;
        let (userinfo, introspection, pending) = overlapping_request(&fixture, state.clone()).await;
        let (first, last) = if userinfo_first {
            (userinfo, introspection)
        } else {
            (introspection, userinfo)
        };
        first.release.notify_one();
        tokio::task::yield_now().await;
        assert!(!pending.is_finished(), "one provider read is insufficient");
        last.release.notify_one();
        let request = pending.await.unwrap().unwrap();
        assert_eq!(request.subject, state.subject);
        assert_eq!(
            request.authorization,
            Authorization::new([Capability::UseOwnSandboxes])
        );
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), 3);
        fixture.assert_unchanged(&state).await;
    }
}

#[tokio::test]
async fn early_introspection_still_requires_matching_complete_active_evidence() {
    for body in [
        json!({"active": true, "sub": "other", "permissions": ["cma:*"]}),
        json!({"active": true, "sub": "owner"}),
        json!({"active": false, "sub": "owner"}),
    ] {
        let fixture = Fixture::new().await;
        let state = fixture.seed().await;
        let inactive = body["active"] == false;
        *fixture.control.introspection.write().await = Reply::json(body);
        let (userinfo, introspection, pending) = overlapping_request(&fixture, state.clone()).await;
        introspection.release.notify_one();
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        userinfo.release.notify_one();
        let result = pending.await.unwrap();
        if inactive {
            let request = result.unwrap();
            assert_eq!(request.authorization, Authorization::none());
            assert!(request.delegated_credential.is_none());
        } else {
            assert_unavailable(result);
        }
        fixture.assert_unchanged(&state).await;
    }
}

#[tokio::test]
async fn overlapping_verification_cannot_authorize_after_token_custody_changes() {
    for case in 0..3 {
        let fixture = Fixture::new().await;
        let state = fixture.seed().await;
        let (userinfo, introspection, pending) = overlapping_request(&fixture, state.clone()).await;
        introspection.release.notify_one();
        let mut changed = state.clone();
        changed.revision += 1;
        match case {
            0 => {
                changed.token_generation += 1;
                changed.tokens.as_mut().unwrap().access_token =
                    SecretValue::new("replacement".into());
            }
            1 => changed.tokens = None,
            _ => changed.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH,
        }
        assert!(fixture
            .store
            .replace_subject(&state.incarnation, state.revision, changed.clone())
            .await
            .unwrap());
        userinfo.release.notify_one();
        assert_unavailable(pending.await.unwrap());
        fixture.assert_unchanged(&changed).await;
    }
}

#[tokio::test]
async fn userinfo_failure_keeps_precedence_and_cancels_pending_introspection() {
    for (status, body, introspection_first) in [
        (StatusCode::UNAUTHORIZED, json!({}), false),
        (StatusCode::UNAUTHORIZED, json!({}), true),
        (StatusCode::OK, json!({"permissions": ["cma:*"]}), false),
    ] {
        let fixture = Fixture::new().await;
        let metadata = fixture.auth.metadata().await.unwrap();
        let userinfo = Arc::new(Gate::default());
        let introspection = Arc::new(Gate::default());
        *fixture.control.userinfo.write().await = Reply {
            status,
            gate: Some(userinfo.clone()),
            ..Reply::json(body)
        };
        *fixture.control.introspection.write().await = Reply {
            status: StatusCode::SERVICE_UNAVAILABLE,
            gate: Some(introspection.clone()),
            ..Reply::json(json!({}))
        };
        let auth = fixture.auth.clone();
        let pending = tokio::spawn(async move {
            identity::verify_subject(&auth, &metadata, "fixture-access").await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                userinfo.reached.notified(),
                introspection.reached.notified()
            );
        })
        .await
        .unwrap();
        if introspection_first {
            introspection.release.notify_one();
            tokio::task::yield_now().await;
            assert!(
                !pending.is_finished(),
                "introspection error cannot preempt userinfo"
            );
        }
        userinfo.release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(2), pending)
            .await
            .expect("userinfo refusal must not wait for pending introspection")
            .unwrap();
        match result {
            Err(crate::auth::verification_error::ProviderVerificationError::Rejected(_)) => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
            }
            Err(crate::auth::verification_error::ProviderVerificationError::Unavailable(
                detail,
            )) => {
                assert_eq!(status, StatusCode::OK);
                assert_eq!(detail, "userinfo response has no subject");
            }
            Ok(_) => panic!("invalid userinfo must never authorize"),
        }
        introspection.release.notify_one();
    }
}
