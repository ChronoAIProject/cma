use super::*;

const RESOURCE: &str = "https://issuer.example/api/v1/proxy/s/cma";

async fn scoped_fixture(owner: &str, resource: Option<&str>) -> (ResourceFixture, String) {
    let mut fixture = ResourceFixture::with_owner(owner).await;
    fixture.identity.auth = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        fixture.identity.store.clone(),
    )
    .with_oauth_api_resource(resource.map(str::to_owned));
    let credential = token(json!({
        "sub":"owner", "token_type":"access", "scope":"openid roles proxy",
        "resources":[RESOURCE], "allowed_service_ids":["cma-connection"],
        "allow_all_services":false, "allowed_node_ids":[], "allow_all_nodes":true
    }));
    *fixture.identity.control.organization_token.write().await = credential.clone();
    *fixture.identity.control.introspection.write().await = Reply::json(json!({
        "active":true, "sub":"owner", "token_type":"access", "exp":future_expiry(),
        "resource":[RESOURCE], "scope":"openid roles proxy", "permissions":["cma:sandbox:use"]
    }));
    (fixture, credential)
}

struct ScopedAccountStanding {
    subject: SubjectId,
    state: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ScopedAccountStanding {
    async fn erasing(
        &self,
        subject: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        if subject != &self.subject {
            return Ok(false);
        }
        match self.state.load(Ordering::SeqCst) {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(layer2_core::account_deletion::AccountDeletionError::Unavailable),
        }
    }
}

async fn scoped_session(fixture: &ResourceFixture, credential: &str) -> (StatusCode, Value) {
    use tower::ServiceExt as _;
    let auth = fixture.identity.auth.clone();
    let app = crate::auth::router(auth.clone()).layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ));
    let response = app
        .oneshot(
            axum::http::Request::get("/auth/session")
                .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 16384)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(credential));
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn restricted_oauth_session_commands_follow_current_account_standing() {
    let (mut fixture, credential) = scoped_fixture("owner", Some(RESOURCE)).await;
    let standing = Arc::new(ScopedAccountStanding {
        subject: SubjectId::new("owner"),
        state: std::sync::atomic::AtomicUsize::new(0),
    });
    let auth = Arc::get_mut(&mut fixture.identity.auth).unwrap();
    auth.set_fixture_standing(standing.clone());
    auth.enable_fixture_account_deletion();
    let (status, active) = scoped_session(&fixture, &credential).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(active["api_commands"], true);
    assert_eq!(active["display_name"], "Live Owner");
    assert_eq!(
        active["capabilities"],
        json!(["use_own_sandboxes", "delete_own_account"])
    );

    standing.state.store(1, Ordering::SeqCst);
    let (status, erasing) = scoped_session(&fixture, &credential).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(erasing["authenticated"], true);
    assert_eq!(erasing["subject"], "owner");
    assert_eq!(erasing["capabilities"], active["capabilities"]);
    assert_eq!(erasing["api_commands"], false);
    for field in ["display_name", "email", "avatar_url"] {
        assert!(erasing.get(field).is_none(), "{field}");
    }

    standing.state.store(2, Ordering::SeqCst);
    let (status, unavailable) = scoped_session(&fixture, &credential).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    for field in ["api_commands", "display_name", "email", "avatar_url"] {
        assert!(unavailable.get(field).is_none(), "{field}");
    }

    standing.state.store(0, Ordering::SeqCst);
    let (status, restored) = scoped_session(&fixture, &credential).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored["api_commands"], true);
}

#[tokio::test]
async fn restricted_oauth_commands_recheck_actor_and_owner_account_standing() {
    for subject in ["owner", "resource-owner"] {
        for state in [1, 2] {
            let (mut fixture, credential) = scoped_fixture("resource-owner", Some(RESOURCE)).await;
            let standing = Arc::new(ScopedAccountStanding {
                subject: SubjectId::new(subject),
                state: std::sync::atomic::AtomicUsize::new(0),
            });
            Arc::get_mut(&mut fixture.identity.auth)
                .unwrap()
                .set_fixture_standing(standing.clone());
            let context = external_context(&fixture, &credential).await;
            let command = admitted(&fixture, &credential, "active-account").await;
            fixture
                .identity
                .auth
                .authorize_resource_command(&fixture.access, &command.receipt.envelope)
                .await
                .unwrap();
            fixture
                .identity
                .auth
                .authorize_command_revival(&fixture.access, &command)
                .await
                .unwrap();

            standing.state.store(state, Ordering::SeqCst);
            let expected = if state == 1 {
                CommandAuthorizationError::Revoked
            } else {
                CommandAuthorizationError::Unavailable
            };
            let before = fixture.identity.control.requests.load(Ordering::SeqCst);
            assert_eq!(
                fixture
                    .identity
                    .auth
                    .command_admission_authority(
                        &context,
                        &credential,
                        &command.receipt.envelope.owner
                    )
                    .await,
                Err(expected.clone()),
            );
            for error in [
                fixture
                    .identity
                    .auth
                    .authorize_resource_command(&fixture.access, &command.receipt.envelope)
                    .await
                    .err()
                    .unwrap(),
                fixture
                    .identity
                    .auth
                    .authorize_command_revival(&fixture.access, &command)
                    .await
                    .err()
                    .unwrap(),
            ] {
                assert!(
                    matches!(error, ResourceCommandAuthorizationError::Custody(error) if error == expected)
                );
            }
            assert_eq!(
                fixture.identity.control.requests.load(Ordering::SeqCst),
                before
            );
            assert!(service(&fixture, fixture.identity.auth.clone())
                .admit(
                    &context,
                    &credential,
                    CommandSubmissionSource::Api,
                    submission("unavailable-account"),
                )
                .await
                .is_err());
            assert_eq!(
                fixture
                    .store
                    .command_work_page(
                        &layer2_core::sandbox::store::PageRequest::new(20, None).unwrap()
                    )
                    .await
                    .unwrap()
                    .items
                    .len(),
                1,
            );
        }
    }
}

#[tokio::test]
async fn restricted_oauth_needs_an_explicit_matching_reader_configuration() {
    for resource in [None, Some("https://other.example/cma"), Some(RESOURCE)] {
        let (fixture, credential) = scoped_fixture("owner", resource).await;
        let allowed = resource == Some(RESOURCE);
        assert_eq!(api_session_commands(&fixture, &credential).await, allowed);
        let context = external_context(&fixture, &credential).await;
        let result = service(&fixture, fixture.identity.auth.clone())
            .admit(
                &context,
                &credential,
                CommandSubmissionSource::Api,
                submission("explicit-resource"),
            )
            .await;
        assert_eq!(result.is_ok(), allowed);
        let work = fixture
            .store
            .command_work_page(&layer2_core::sandbox::store::PageRequest::new(20, None).unwrap())
            .await
            .unwrap();
        assert_eq!(work.items.len(), usize::from(allowed));
    }
}

#[tokio::test]
async fn restricted_oauth_rechecks_live_evidence_for_admission_execution_and_revival() {
    for change in [
        json!({"resource":[]}),
        json!({"resource":["https://other.example/cma"]}),
        json!({"scope":"openid roles"}),
        json!({"permissions":[]}),
        json!({"sub":"another-tenant"}),
        json!({"exp":1}),
        json!({"active":false}),
        json!({"outage":true}),
    ] {
        let (fixture, credential) = scoped_fixture("owner", Some(RESOURCE)).await;
        let context = external_context(&fixture, &credential).await;
        let command = admitted(&fixture, &credential, "before-withdrawal").await;
        fixture
            .identity
            .auth
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .unwrap();
        fixture
            .identity
            .auth
            .authorize_command_revival(&fixture.access, &command)
            .await
            .unwrap();
        {
            let mut reply = fixture.identity.control.introspection.write().await;
            if change["outage"] == true {
                reply.status = StatusCode::SERVICE_UNAVAILABLE;
            } else {
                let mut body: Value = serde_json::from_str(&reply.body).unwrap();
                body.as_object_mut()
                    .unwrap()
                    .extend(change.as_object().unwrap().clone());
                reply.body = body.to_string();
            }
        }
        assert!(
            service(&fixture, fixture.identity.auth.clone())
                .admit(
                    &context,
                    &credential,
                    CommandSubmissionSource::Api,
                    submission("after-withdrawal"),
                )
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            fixture
                .identity
                .auth
                .authorize_resource_command(&fixture.access, &command.receipt.envelope)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            fixture
                .identity
                .auth
                .authorize_command_revival(&fixture.access, &command)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            fixture
                .store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command)
        );
        assert_eq!(
            fixture
                .store
                .command_work_page(
                    &layer2_core::sandbox::store::PageRequest::new(20, None).unwrap()
                )
                .await
                .unwrap()
                .items
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn restricted_oauth_preserves_shared_resource_membership_fences() {
    for owner in ["resource-owner", "org"] {
        let (fixture, credential) = scoped_fixture(owner, Some(RESOURCE)).await;
        let context = external_context(&fixture, &credential).await;
        let command = admitted(&fixture, &credential, "authorized-member").await;
        *fixture.identity.control.organization.write().await =
            Reply::json(json!({"id":"org", "your_role":"viewer"}));
        assert!(service(&fixture, fixture.identity.auth.clone())
            .admit(
                &context,
                &credential,
                CommandSubmissionSource::Api,
                submission("former-member"),
            )
            .await
            .is_err());
        assert!(fixture
            .identity
            .auth
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .is_err());
        assert!(fixture
            .identity
            .auth
            .authorize_command_revival(&fixture.access, &command)
            .await
            .is_err());
        assert_eq!(
            fixture
                .store
                .command_work_page(
                    &layer2_core::sandbox::store::PageRequest::new(20, None).unwrap()
                )
                .await
                .unwrap()
                .items
                .len(),
            1
        );
    }
}
