use super::*;
use base64::Engine as _;

#[path = "owner_service_access_tests.rs"]
mod owner_service_access;
#[path = "api_bearer_refusal_tests.rs"]
mod refusals;
#[path = "scoped_oauth_tests.rs"]
mod scoped_oauth;

fn token(claims: Value) -> String {
    format!(
        "e30.{}.fixture-signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap())
    )
}

fn ordinary_token() -> String {
    // Decoded exp is deliberately expired: only verified introspection supplies expiry.
    token(json!({"sub":"owner", "token_type":"access", "exp":1}))
}

async fn provider_expiry(fixture: &ResourceFixture, seconds: u64) {
    *fixture.identity.control.introspection.write().await = Reply::json(json!({
        "active":true, "sub":"owner", "token_type":"access", "exp":seconds,
        "permissions":["cma:sandbox:use"],
    }));
}

fn future_expiry() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

async fn external_context(fixture: &ResourceFixture, token: &str) -> CallerContext {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let request = fixture
        .identity
        .auth
        .resolve_request(&headers)
        .await
        .unwrap()
        .unwrap();
    fixture.identity.auth.resource_request_context(&request)
}

fn submission(key: &str) -> CommandSubmission {
    CommandSubmission {
        agent_id: layer2_core::AgentId("agent".into()),
        command: AgentCommand::message("independent API input"),
        idempotency_key: key.into(),
        expected_agent_revision: None,
        requested_model: None,
    }
}

fn service(fixture: &ResourceFixture, auth: Arc<AuthState>) -> Arc<AgentCommandService> {
    AgentCommandService::new(
        fixture.store.clone(),
        Arc::new(fixture.access.clone()),
        auth,
    )
}

async fn admitted(fixture: &ResourceFixture, credential: &str, key: &str) -> AdmittedAgentCommand {
    let context = external_context(fixture, credential).await;
    let view = service(fixture, fixture.identity.auth.clone())
        .admit(
            &context,
            credential,
            CommandSubmissionSource::Api,
            submission(key),
        )
        .await
        .unwrap();
    fixture
        .store
        .get_command(&view.receipt.command_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn independent_bearer_admission_restart_and_revival_preserve_existing_login_and_receipt() {
    for owner in ["owner", "resource-owner", "org"] {
        let fixture = ResourceFixture::with_owner(owner).await;
        let login = fixture
            .identity
            .store
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .unwrap();
        let credential = ordinary_token();
        *fixture.identity.control.organization_token.write().await = credential.clone();
        provider_expiry(&fixture, future_expiry()).await;
        let command = admitted(&fixture, &credential, "independent-request").await;
        assert!(command
            .receipt
            .envelope
            .authorization
            .authority_id
            .starts_with("api:"));
        assert_eq!(command.receipt.envelope.owner, fixture.envelope.owner);
        assert!(matches!(
            command.receipt.envelope.actor,
            AgentActor::Api {
                delegation_id: None,
                ..
            }
        ));
        fixture.identity.assert_unchanged(&login).await;
        // A restarted auth adapter recovers only the exact saved API credential.
        let restarted = AuthState::with_store(
            fixture.identity.auth.fixture_mode().clone(),
            None,
            fixture.identity.store.clone(),
        );
        let context = external_context(&fixture, &credential).await;
        let replay = service(&fixture, restarted.clone())
            .admit(
                &context,
                &credential,
                CommandSubmissionSource::Api,
                submission("independent-request"),
            )
            .await
            .unwrap();
        assert_eq!(
            replay.receipt.command_id,
            command.receipt.envelope.command_id
        );
        assert_eq!(replay.receipt.response_id, command.receipt.response_id);
        restarted
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .unwrap();
        restarted
            .authorize_command_revival(&fixture.access, &command)
            .await
            .unwrap()
            .require_command(&command)
            .unwrap();
        fixture.identity.assert_unchanged(&login).await;
        // Existing personal authority still works using its own custody.
        if owner == "owner" {
            restarted
                .authorize_command(&fixture.envelope)
                .await
                .unwrap();
            // Replacing browser sign-in does not replace or refresh the API record.
            fixture.identity.seed().await;
            assert!(restarted
                .authorize_command(&fixture.envelope)
                .await
                .is_err());
            restarted
                .authorize_resource_command(&fixture.access, &command.receipt.envelope)
                .await
                .unwrap();
        }
        let serialized = serde_json::to_string(&command).unwrap();
        for secret in [
            &credential,
            "must-not-leak-refresh-token",
            "must-not-leak-access-token",
        ] {
            assert!(!serialized.contains(secret));
        }
    }
}

#[tokio::test]
async fn cookie_resolution_never_acquires_independent_custody_even_with_a_bearer_header() {
    use crate::auth::persistence::{BrowserLogin, SecretDigest};
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    provider_expiry(&fixture, future_expiry()).await;
    fixture
        .identity
        .store
        .put_browser_login(BrowserLogin {
            id: SecretDigest::identify("browser-login", "existing-cookie"),
            subject: SubjectId::new("owner"),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        })
        .await
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "cma_session=existing-cookie".parse().unwrap(),
    );
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {credential}").parse().unwrap(),
    );
    let request = fixture
        .identity
        .auth
        .resolve_request(&headers)
        .await
        .unwrap()
        .unwrap();
    let context = fixture.identity.auth.resource_request_context(&request);
    assert_eq!(context.credential_source, CredentialSource::Retained);
    let selected = request.delegated_credential.as_deref().unwrap();
    assert_eq!(selected, "must-not-leak-access-token");
    let authority = fixture
        .identity
        .auth
        .command_admission_authority(&context, selected, &fixture.envelope.owner)
        .await
        .unwrap();
    assert_eq!(authority, fixture.envelope.authorization);
    assert!(fixture
        .identity
        .auth
        .command_admission_authority(&context, &credential, &fixture.envelope.owner)
        .await
        .is_err());
}

#[tokio::test]
async fn exact_api_token_is_revalidated_for_execution_revival_and_current_membership() {
    for failure in ["revoked", "permissions", "subject", "expiry", "membership"] {
        let fixture = ResourceFixture::new(false).await;
        let credential = ordinary_token();
        *fixture.identity.control.organization_token.write().await = credential.clone();
        provider_expiry(&fixture, future_expiry()).await;
        let command = admitted(&fixture, &credential, failure).await;
        match failure {
            "revoked" => {
                *fixture.identity.control.introspection.write().await =
                    Reply::json(json!({"active":false,"sub":"owner"}))
            }
            "permissions" => {
                *fixture.identity.control.introspection.write().await = Reply::json(
                    json!({"active":true,"sub":"owner","token_type":"access","exp":future_expiry(),"permissions":[]}),
                )
            }
            "subject" => {
                *fixture.identity.control.userinfo.write().await =
                    Reply::json(json!({"sub":"another-subject","permissions":["cma:*"]}))
            }
            "expiry" => provider_expiry(&fixture, 1).await,
            "membership" => {
                *fixture.identity.control.organization.write().await =
                    Reply::json(json!({"id":"org","your_role":"viewer"}))
            }
            _ => unreachable!(),
        }
        assert!(
            fixture
                .identity
                .auth
                .authorize_resource_command(&fixture.access, &command.receipt.envelope)
                .await
                .is_err(),
            "{failure}"
        );
        assert!(
            fixture
                .identity
                .auth
                .authorize_command_revival(&fixture.access, &command)
                .await
                .is_err(),
            "{failure}"
        );
        assert_eq!(
            fixture
                .store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap(),
            Some(command)
        );
    }
}

#[tokio::test]
async fn api_execution_and_revival_proofs_expire_with_the_credential() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    let expires = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3;
    provider_expiry(&fixture, expires).await;
    let command = admitted(&fixture, &credential, "short-lived").await;
    let execution = fixture
        .identity
        .auth
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap();
    let revival = fixture
        .identity
        .auth
        .authorize_command_revival(&fixture.access, &command)
        .await
        .unwrap();
    tokio::time::sleep(
        (SystemTime::UNIX_EPOCH + Duration::from_secs(expires))
            .duration_since(SystemTime::now())
            .unwrap()
            + Duration::from_millis(10),
    )
    .await;
    assert!(execution.fence().require_current().is_err());
    assert!(revival.require_current().is_err());
    let before = fixture.identity.control.requests.load(Ordering::SeqCst);
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
        fixture.identity.control.requests.load(Ordering::SeqCst),
        before
    );
}

#[tokio::test]
async fn third_party_cma_scoped_oauth_admits_followups_and_live_revocation_stops_them() {
    const RESOURCE: &str = "https://issuer.example/api/v1/proxy/s/cma";
    let mut fixture = ResourceFixture::with_owner("owner").await;
    fixture.identity.auth = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        fixture.identity.store.clone(),
    )
    .with_oauth_api_resource(Some(RESOURCE.into()));
    let credential = token(
        json!({"sub":"owner","token_type":"access","scope":"openid roles proxy",
        "client_id":"external-web-client","resources":[RESOURCE],"allowed_service_ids":["cma-connection"],
        "allow_all_services":false,"allowed_node_ids":[],"allow_all_nodes":true}),
    );
    *fixture.identity.control.introspection.write().await = Reply::json(json!({
        "active":true,"sub":"owner","token_type":"access","exp":future_expiry(),
        "resource":[RESOURCE],"scope":"openid roles proxy","permissions":["cma:sandbox:use"]
    }));
    assert_eq!(api_session_commands(&fixture, &credential).await, true);
    let first = admitted(&fixture, &credential, "third-party-first").await;
    let second = admitted(&fixture, &credential, "third-party-second").await;
    assert_ne!(
        first.receipt.envelope.command_id,
        second.receipt.envelope.command_id
    );
    fixture
        .identity
        .auth
        .authorize_resource_command(&fixture.access, &second.receipt.envelope)
        .await
        .unwrap();
    *fixture.identity.control.introspection.write().await = Reply::json(json!({"active":false}));
    assert!(fixture
        .identity
        .auth
        .authorize_resource_command(&fixture.access, &second.receipt.envelope)
        .await
        .is_err());
}

#[tokio::test]
async fn scoped_oauth_cannot_substitute_identity_or_wrong_resource_evidence() {
    const RESOURCE: &str = "https://issuer.example/api/v1/proxy/s/cma";
    for observed in [json!([]), json!(["other-resource"])] {
        let mut fixture = ResourceFixture::with_owner("owner").await;
        fixture.identity.auth = AuthState::with_store(
            fixture.identity.auth.fixture_mode().clone(),
            None,
            fixture.identity.store.clone(),
        )
        .with_oauth_api_resource(Some(RESOURCE.into()));
        let credential = token(json!({"sub":"owner","token_type":"access","scope":"proxy",
            "resources":[RESOURCE],"allowed_service_ids":["cma-connection"],"allow_all_services":false,
            "allowed_node_ids":[],"allow_all_nodes":true}));
        *fixture.identity.control.introspection.write().await = Reply::json(json!({
            "active":true,"sub":"owner","token_type":"access","exp":future_expiry(),
            "resource":observed,"scope":"proxy","permissions":["cma:sandbox:use"]
        }));
        assert_eq!(api_session_commands(&fixture, &credential).await, false);
        let context = external_context(&fixture, &credential).await;
        assert!(fixture
            .identity
            .auth
            .command_admission_authority(&context, &credential, &fixture.envelope.owner)
            .await
            .is_err());
    }
}

async fn api_session_commands(fixture: &ResourceFixture, credential: &str) -> Value {
    use tower::ServiceExt;
    let auth = fixture.identity.auth.clone();
    let app = crate::auth::router(auth.clone()).layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::middleware::attach_subject,
    ));
    let response = app
        .oneshot(
            axum::http::Request::get("/auth/session")
                .header("authorization", format!("Bearer {credential}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 16384)
        .await
        .unwrap();
    serde_json::from_slice::<Value>(&body).unwrap()["api_commands"].clone()
}
