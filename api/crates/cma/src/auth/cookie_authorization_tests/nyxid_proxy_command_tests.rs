//! Commands use durable proxy admission, never recast identity as human consent.
use super::*;
use crate::auth::nyxid_proxy::ProxyGrant;
use layer2_core::sandbox::store::PageRequest;
use sha2::{Digest as _, Sha256};

#[path = "nyxid_proxy_sandbox_control_tests.rs"]
mod sandbox_controls;

const ISSUER: &str = "https://nyx.example.test";
const SERVICE: &str = "cma-service";
const OWNER_TOKEN: &str = "must-not-leak-access-token";

fn proxy_auth(fixture: &ResourceFixture) -> Arc<AuthState> {
    AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        fixture.identity.store.clone(),
    )
    .with_agent_key_service(Some(SERVICE.into()))
    .with_identity_proxy(
        true,
        Some(ISSUER.into()),
        Some(format!("{ISSUER}/.well-known/jwks.json")),
        None,
    )
    .unwrap()
}

fn grant_id(grant: &ProxyGrant) -> [u8; 32] {
    Sha256::digest(
        serde_json::to_vec(&(
            "nyxid-proxy-admission-v1",
            &grant.issuer,
            &grant.service_id,
            &grant.subject,
            &grant.assertion_id,
        ))
        .unwrap(),
    )
    .into()
}

async fn seed_grant(fixture: &ResourceFixture) -> ProxyGrant {
    let owner = fixture
        .identity
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    // Signature ingress is exercised separately by jwt_tests. This fixture
    // begins at the immutable admission record created after verification.
    let mut grant = ProxyGrant {
        id: [0; 32],
        issuer: ISSUER.into(),
        service_id: SERVICE.into(),
        assertion_id: "verified-proxy-assertion".into(),
        subject: owner.subject,
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        owner_incarnation: owner.incarnation,
    };
    grant.id = grant_id(&grant);
    assert!(fixture
        .identity
        .store
        .insert_proxy_grant(grant.clone())
        .await
        .unwrap());
    grant
}

fn context(auth: &AuthState, grant: &ProxyGrant, credential: &str) -> CallerContext {
    auth.resource_request_context(&RequestAuth {
        binding: None,
        credential_source: CredentialSource::IdentityProxy {
            grant_digest: grant.id,
        },
        subject: grant.subject.clone(),
        authorization: grant.authorization.clone(),
        profile: Profile::default(),
        delegated_credential: Some(credential.into()),
    })
}

fn submission(key: &str) -> CommandSubmission {
    CommandSubmission {
        agent_id: layer2_core::AgentId("agent".into()),
        command: AgentCommand::message("proxy-admitted message"),
        idempotency_key: key.into(),
        expected_agent_revision: None,
        requested_model: None,
    }
}

fn service(fixture: &ResourceFixture, auth: Arc<AuthState>) -> Arc<AgentCommandService> {
    AgentCommandService::new(
        fixture.store.clone(),
        Arc::new(ResourceAccessService::new(
            fixture.store.clone(),
            auth.clone(),
        )),
        auth,
    )
}

async fn admitted(
    fixture: &ResourceFixture,
    auth: Arc<AuthState>,
    grant: &ProxyGrant,
) -> AdmittedAgentCommand {
    let context = context(&auth, grant, OWNER_TOKEN);
    assert_eq!(
        context.credential_source,
        CredentialSource::IdentityProxy {
            grant_digest: grant.id
        }
    );
    let accepted = service(fixture, auth)
        .admit(
            &context,
            OWNER_TOKEN,
            CommandSubmissionSource::Api,
            submission("proxy-request"),
        )
        .await
        .unwrap();
    fixture
        .store
        .get_command(&accepted.receipt.command_id)
        .await
        .unwrap()
        .unwrap()
}

async fn seed_assertion_grant(fixture: &ResourceFixture, assertion: &str) -> ProxyGrant {
    let owner = fixture
        .identity
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    let mut grant = ProxyGrant {
        id: [0; 32],
        issuer: ISSUER.into(),
        service_id: SERVICE.into(),
        assertion_id: assertion.into(),
        subject: owner.subject,
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
        owner_incarnation: owner.incarnation,
    };
    grant.id = grant_id(&grant);
    assert!(fixture
        .identity
        .store
        .insert_proxy_grant(grant.clone())
        .await
        .unwrap());
    grant
}

async fn admit_with(
    fixture: &ResourceFixture,
    auth: Arc<AuthState>,
    grant: &ProxyGrant,
    key: &str,
) -> layer2_core::ids::AgentCommandId {
    service(fixture, auth.clone())
        .admit(
            &context(&auth, grant, OWNER_TOKEN),
            OWNER_TOKEN,
            CommandSubmissionSource::Api,
            submission(key),
        )
        .await
        .unwrap()
        .receipt
        .command_id
}

#[tokio::test]
async fn exact_key_retry_under_a_fresh_proxy_assertion_replays_only_with_the_gated_writer() {
    // NyxID signs a new assertion (new jti) for every proxied request, so the
    // exact grant changes between an original request and its retry.
    let fixture = ResourceFixture::with_owner("owner").await;
    let legacy = proxy_auth(&fixture);
    let first = seed_assertion_grant(&fixture, "assertion-legacy-1").await;
    let retry = seed_assertion_grant(&fixture, "assertion-legacy-2").await;
    assert_ne!(first.id, retry.id);
    let original = admit_with(&fixture, legacy.clone(), &first, "legacy-key").await;
    let duplicate = admit_with(&fixture, legacy, &retry, "legacy-key").await;
    assert_ne!(
        original, duplicate,
        "legacy writers keep the per-assertion scope until the gate is enabled"
    );
    let legacy_command = fixture.store.get_command(&original).await.unwrap().unwrap();
    assert_eq!(
        legacy_command
            .receipt
            .envelope
            .authorization
            .replay_principal,
        None
    );

    let gated = proxy_auth(&fixture).with_proxy_replay_principal(true);
    let first = seed_assertion_grant(&fixture, "assertion-gated-1").await;
    let retry = seed_assertion_grant(&fixture, "assertion-gated-2").await;
    let original = admit_with(&fixture, gated.clone(), &first, "gated-key").await;
    let replayed = admit_with(&fixture, gated.clone(), &retry, "gated-key").await;
    assert_eq!(
        original, replayed,
        "the retry must replay the original command"
    );
    let command = fixture.store.get_command(&original).await.unwrap().unwrap();
    let authorization = &command.receipt.envelope.authorization;
    // Execution keeps the exact original grant; only replay uses the principal.
    assert_eq!(
        authorization.authority_id,
        format!(
            "nyxid-proxy:{}",
            first
                .id
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    );
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "nyxid-proxy-replay-principal-v1",
            &first.issuer,
            &first.service_id,
            &first.subject,
            &first.owner_incarnation,
        ))
        .unwrap(),
    );
    assert_eq!(
        authorization.replay_principal,
        Some(format!("nyxid-proxy-principal:{digest:x}"))
    );
    // A different key from the same principal is still a distinct command.
    let other = admit_with(&fixture, gated.clone(), &retry, "gated-other-key").await;
    assert_ne!(original, other);
    // The recorded principal remains accepted for execution after a restart.
    proxy_auth(&fixture)
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap()
        .fence()
        .require_envelope(&command.receipt.envelope)
        .unwrap();
}

#[tokio::test]
async fn retained_proxy_command_accepts_only_its_derived_replay_principal() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let auth = proxy_auth(&fixture);
    let grant = seed_grant(&fixture).await;
    let mut command = admitted(&fixture, auth, &grant).await;
    assert_eq!(
        command.receipt.envelope.authorization.replay_principal,
        None
    );
    let expected_authority = format!(
        "nyxid-proxy:{}",
        grant
            .id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert_eq!(
        serde_json::to_value(&command.receipt.envelope.authorization).unwrap(),
        json!({"authority_id": expected_authority, "policy_revision": 1, "grant_revision": null})
    );
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "nyxid-proxy-replay-principal-v1",
            &grant.issuer,
            &grant.service_id,
            &grant.subject,
            &grant.owner_incarnation,
        ))
        .unwrap(),
    );
    command.receipt.envelope.authorization.replay_principal =
        Some(format!("nyxid-proxy-principal:{digest:x}"));
    let restarted = proxy_auth(&fixture);
    restarted
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap()
        .fence()
        .require_envelope(&command.receipt.envelope)
        .unwrap();
    restarted
        .authorize_command_revival(&fixture.access, &command)
        .await
        .unwrap()
        .require_command(&command)
        .unwrap();

    command.receipt.envelope.authorization.replay_principal = Some("tampered".into());
    assert!(restarted
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .is_err());
    assert!(restarted
        .authorize_command_revival(&fixture.access, &command)
        .await
        .is_err());
}

#[tokio::test]
async fn proxy_command_admission_restart_and_revival_preserve_proxy_authority_and_receipt() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let auth = proxy_auth(&fixture);
    let grant = seed_grant(&fixture).await;
    let original_owner = fixture
        .identity
        .store
        .subject(&grant.subject)
        .await
        .unwrap()
        .unwrap();
    let command = admitted(&fixture, auth.clone(), &grant).await;
    let envelope = &command.receipt.envelope;
    assert!(envelope
        .authorization
        .authority_id
        .starts_with("nyxid-proxy:"));
    assert_eq!(envelope.source.kind, AgentSourceKind::Api);
    assert_eq!(
        envelope.owner,
        AgentOwner::Person {
            subject: grant.subject.clone()
        }
    );
    assert_eq!(
        envelope.actor,
        AgentActor::Api {
            subject: grant.subject.clone(),
            delegation_id: None
        }
    );

    // Only persisted grant and exact owner incarnation survive this adapter
    // restart. No original JWT or in-memory request is available to the worker.
    let restarted = proxy_auth(&fixture);
    let custody = restarted
        .retained_proxy_custody(&grant.subject, &envelope.authorization)
        .await
        .unwrap();
    let request = restarted.verify_proxy_custody(&custody).await.unwrap();
    assert_eq!(
        request.credential_source,
        CredentialSource::IdentityProxy {
            grant_digest: grant.id
        }
    );
    assert_eq!(
        restarted
            .resource_request_context(&request)
            .credential_source,
        request.credential_source
    );
    let proof = restarted
        .authorize_resource_command(&fixture.access, envelope)
        .await
        .unwrap();
    proof.fence().require_envelope(envelope).unwrap();
    restarted
        .authorize_command_revival(&fixture.access, &command)
        .await
        .unwrap()
        .require_command(&command)
        .unwrap();
    let again = service(&fixture, restarted.clone())
        .admit(
            &context(&restarted, &grant, OWNER_TOKEN),
            OWNER_TOKEN,
            CommandSubmissionSource::Api,
            submission("proxy-request"),
        )
        .await
        .unwrap();
    assert_eq!(again.receipt.command_id, envelope.command_id);
    assert_eq!(again.receipt.response_id, command.receipt.response_id);
    fixture.identity.assert_unchanged(&original_owner).await;

    let audits = fixture
        .store
        .command_authorization_audits(&envelope.command_id, &PageRequest::new(20, None).unwrap())
        .await
        .unwrap();
    assert_eq!(audits.items.len(), 1);
    assert_eq!(audits.items[0].credential_authority, envelope.authorization);
    for serialized in [
        serde_json::to_string(&command).unwrap(),
        serde_json::to_string(&grant).unwrap(),
        serde_json::to_string(&audits.items).unwrap(),
    ] {
        for secret in [
            OWNER_TOKEN,
            "must-not-leak-refresh-token",
            "must-not-leak-client-secret",
        ] {
            assert!(
                !serialized.contains(secret),
                "credential must not enter durable command or grant records"
            );
        }
    }
}

#[tokio::test]
async fn provider_permission_growth_cannot_expand_the_original_proxy_ceiling() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let auth = proxy_auth(&fixture);
    let grant = seed_grant(&fixture).await;
    let command = admitted(&fixture, auth.clone(), &grant).await;
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:*"]}));
    let restarted = proxy_auth(&fixture);
    let caller = restarted
        .authorize_command(&command.receipt.envelope)
        .await
        .unwrap();
    assert_eq!(caller.authorization, grant.authorization);
    assert!(!caller.authorization.allows(Capability::ManageOwnBots));
    assert!(!caller.authorization.allows(Capability::CreateSandbox));
    let proof = restarted
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap();
    assert_eq!(proof.caller().authorization, grant.authorization);

    let mut broadened = grant.clone();
    broadened.authorization = crate::permissions::capabilities_from_permissions(["cma:*"]);
    assert!(!fixture
        .identity
        .store
        .insert_proxy_grant(broadened)
        .await
        .unwrap());
    assert_eq!(
        fixture.identity.store.proxy_grant(&grant.id).await.unwrap(),
        Some(grant.clone())
    );
    assert_eq!(
        restarted
            .authorize_command(&command.receipt.envelope)
            .await
            .unwrap()
            .authorization,
        grant.authorization
    );
}

#[tokio::test]
async fn loss_of_owner_credentials_and_replacement_sign_in_revoke_old_proxy_commands() {
    for replacement in [false, true] {
        let fixture = ResourceFixture::with_owner("owner").await;
        let auth = proxy_auth(&fixture);
        let grant = seed_grant(&fixture).await;
        let command = admitted(&fixture, auth.clone(), &grant).await;
        if replacement {
            let next = fixture.identity.seed().await;
            assert_ne!(next.incarnation, grant.owner_incarnation);
        } else {
            let mut owner = fixture
                .identity
                .store
                .subject(&grant.subject)
                .await
                .unwrap()
                .unwrap();
            owner.tokens = None;
            owner.token_generation += 1;
            owner.revision += 1;
            fixture.identity.store.put_subject(owner).await.unwrap();
        }
        let restarted = proxy_auth(&fixture);
        assert!(restarted
            .authorize_command(&command.receipt.envelope)
            .await
            .is_err());
        assert!(restarted
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .is_err());
        assert!(restarted
            .authorize_command_revival(&fixture.access, &command)
            .await
            .is_err());
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
async fn current_provider_permission_removal_refuses_proxy_execution_and_revival() {
    for inactive in [false, true] {
        let fixture = ResourceFixture::with_owner("owner").await;
        let auth = proxy_auth(&fixture);
        let grant = seed_grant(&fixture).await;
        let command = admitted(&fixture, auth.clone(), &grant).await;
        *fixture.identity.control.introspection.write().await =
            Reply::json(json!({"active":!inactive,"sub":"owner","permissions":[]}));
        assert!(auth
            .authorize_resource_command(&fixture.access, &command.receipt.envelope)
            .await
            .is_err());
        assert!(auth
            .authorize_command_revival(&fixture.access, &command)
            .await
            .is_err());
        assert!(auth
            .authorize_command(&command.receipt.envelope)
            .await
            .is_err());
        assert!(service(&fixture, auth.clone())
            .admit(
                &context(&auth, &grant, OWNER_TOKEN),
                OWNER_TOKEN,
                CommandSubmissionSource::Api,
                submission("after-revocation"),
            )
            .await
            .is_err());
    }
}

#[tokio::test]
async fn malformed_and_foreign_grants_cannot_borrow_matching_owner_credentials() {
    for foreign in ["issuer", "service", "subject", "incarnation", "digest"] {
        let fixture = ResourceFixture::with_owner("owner").await;
        let auth = proxy_auth(&fixture);
        let original = seed_grant(&fixture).await;
        let mut grant = original.clone();
        grant.assertion_id = format!("foreign-{foreign}");
        match foreign {
            "issuer" => grant.issuer = "https://another-issuer.example".into(),
            "service" => grant.service_id = "another-service".into(),
            "subject" => grant.subject = SubjectId::new("another-owner"),
            "incarnation" => grant.owner_incarnation = "another-incarnation".into(),
            "digest" => {}
            _ => unreachable!(),
        }
        grant.id = grant_id(&grant);
        if foreign == "digest" {
            grant.id[0] ^= 1;
            assert!(fixture
                .identity
                .store
                .insert_proxy_grant(grant)
                .await
                .is_err());
            continue;
        }
        assert!(fixture
            .identity
            .store
            .insert_proxy_grant(grant.clone())
            .await
            .unwrap());
        let mut request = context(&auth, &original, OWNER_TOKEN);
        request.credential_source = CredentialSource::IdentityProxy {
            grant_digest: grant.id,
        };
        assert!(
            service(&fixture, auth)
                .admit(
                    &request,
                    OWNER_TOKEN,
                    CommandSubmissionSource::Api,
                    submission("foreign-request"),
                )
                .await
                .is_err(),
            "foreign {foreign} must not provide command custody"
        );
    }
}

#[tokio::test]
async fn altered_command_authority_or_actor_cannot_reuse_proxy_custody() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let auth = proxy_auth(&fixture);
    let grant = seed_grant(&fixture).await;
    let command = admitted(&fixture, auth.clone(), &grant).await;
    for change in [
        "policy",
        "grant_revision",
        "missing",
        "malformed",
        "human",
        "foreign_actor",
    ] {
        let mut envelope = command.receipt.envelope.clone();
        match change {
            "policy" => envelope.authorization.policy_revision += 1,
            "grant_revision" => envelope.authorization.grant_revision = Some(1),
            "missing" => {
                envelope.authorization.authority_id = format!("nyxid-proxy:{}", "00".repeat(32))
            }
            "malformed" => envelope.authorization.authority_id = "nyxid-proxy:invalid".into(),
            "human" => {
                envelope.actor = AgentActor::Human {
                    subject: grant.subject.clone(),
                }
            }
            "foreign_actor" => {
                envelope.actor = AgentActor::Api {
                    subject: SubjectId::new("another-owner"),
                    delegation_id: None,
                }
            }
            _ => unreachable!(),
        }
        assert!(auth.authorize_command(&envelope).await.is_err(), "{change}");
        assert!(
            auth.authorize_resource_command(&fixture.access, &envelope)
                .await
                .is_err(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn proxy_admission_rejects_wrong_credential_or_permissions_outside_saved_grant() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let auth = proxy_auth(&fixture);
    let grant = seed_grant(&fixture).await;
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:*"]}));
    for change in ["credential", "capabilities", "grant", "human"] {
        let mut context = context(&auth, &grant, OWNER_TOKEN);
        let mut credential = OWNER_TOKEN;
        match change {
            "credential" => credential = "unrelated-owner-credential",
            "capabilities" => {
                context.caller.authorization =
                    crate::permissions::capabilities_from_permissions(["cma:*"])
            }
            "grant" => {
                context.credential_source = CredentialSource::IdentityProxy {
                    grant_digest: [0; 32],
                }
            }
            "human" => {
                context.actor = AgentActor::Human {
                    subject: grant.subject.clone(),
                }
            }
            _ => unreachable!(),
        }
        assert!(
            service(&fixture, auth.clone())
                .admit(
                    &context,
                    credential,
                    CommandSubmissionSource::Api,
                    submission(change),
                )
                .await
                .is_err(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn owner_token_rotation_preserves_proxy_grant_but_disabling_proxy_auth_revokes_it() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let auth = proxy_auth(&fixture);
    let grant = seed_grant(&fixture).await;
    let command = admitted(&fixture, auth.clone(), &grant).await;
    let mut owner = fixture
        .identity
        .store
        .subject(&grant.subject)
        .await
        .unwrap()
        .unwrap();
    owner.tokens.as_mut().unwrap().access_token = SecretValue::new("rotated-owner-token".into());
    owner.token_generation += 1;
    owner.revision += 1;
    fixture.identity.store.put_subject(owner).await.unwrap();
    let restarted = proxy_auth(&fixture);
    let custody = restarted
        .retained_proxy_custody(&grant.subject, &command.receipt.envelope.authorization)
        .await
        .unwrap();
    let request = restarted.verify_proxy_custody(&custody).await.unwrap();
    assert_eq!(
        request.credential_source,
        CredentialSource::IdentityProxy {
            grant_digest: grant.id
        }
    );
    assert_eq!(
        request.delegated_credential.as_deref(),
        Some("rotated-owner-token")
    );
    restarted
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap();
    let disabled = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        fixture.identity.store.clone(),
    )
    .with_agent_key_service(Some(SERVICE.into()));
    assert!(disabled
        .authorize_command(&command.receipt.envelope)
        .await
        .is_err());
    assert!(disabled
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .is_err());
}

struct SignedIngress {
    auth: Arc<AuthState>,
    server: tokio::task::JoinHandle<()>,
}

impl SignedIngress {
    async fn new(fixture: &ResourceFixture) -> Self {
        Self::with_audience(fixture, None, None).await
    }
    async fn with_audience(
        fixture: &ResourceFixture,
        public_url: Option<&str>,
        audience: Option<String>,
    ) -> Self {
        use crate::auth::nyxid_proxy::jwt::Signer;
        let keys = json!({"keys":[Signer::primary().key("proxy-test-key")]}).to_string();
        let app = Router::new().route(
            "/jwks",
            get(move || {
                let keys = keys.clone();
                async move { ([("content-type", "application/json")], keys) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let jwks_url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut mode = fixture.identity.auth.fixture_mode().clone();
        if let (
            AuthMode::Provider {
                public_url: current,
                ..
            },
            Some(public_url),
        ) = (&mut mode, public_url)
        {
            *current = public_url.into();
        }
        let auth = AuthState::with_store(mode, None, fixture.identity.store.clone())
            .with_agent_key_service(Some(SERVICE.into()))
            .with_identity_proxy(true, Some(ISSUER.into()), Some(jwks_url), audience)
            .unwrap();
        Self { auth, server }
    }
}

impl Drop for SignedIngress {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn signed_claims() -> Value {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    json!({
        "sub":"owner", "iss":ISSUER, "aud":"https://cma.example.test",
        "iat":now, "exp":now+60, "jti":uuid::Uuid::new_v4().to_string(),
        "nyx_service_id":SERVICE, "permissions":["cma:sandbox:use"],
        "name":"Signed owner", "roles":[], "groups":[],
    })
}

fn signed_headers(claims: &Value) -> HeaderMap {
    use crate::auth::nyxid_proxy::jwt::{Signer, HEADER};
    let mut headers = HeaderMap::new();
    headers.insert(
        HEADER,
        Signer::primary()
            .token(claims, "proxy-test-key")
            .parse()
            .unwrap(),
    );
    headers
}

#[tokio::test]
async fn signed_identity_header_resolves_into_real_write_and_restarted_worker_authority() {
    let fixture = ResourceFixture::with_owner("owner").await;
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:*"]}));
    let ingress = SignedIngress::new(&fixture).await;
    let headers = signed_headers(&signed_claims());
    let request = ingress
        .auth
        .resolve_request(&headers)
        .await
        .unwrap()
        .unwrap();
    let CredentialSource::IdentityProxy { grant_digest } = request.credential_source else {
        panic!("signed proxy ingress must preserve its distinct authority");
    };
    assert_eq!(request.subject, SubjectId::new("owner"));
    assert_eq!(
        request.authorization,
        Authorization::new([Capability::UseOwnSandboxes])
    );
    let context = ingress.auth.resource_request_context(&request);
    let accepted = service(&fixture, ingress.auth.clone())
        .admit(
            &context,
            request.delegated_credential.as_deref().unwrap(),
            CommandSubmissionSource::Api,
            submission("signed-write"),
        )
        .await
        .unwrap();
    let command = fixture
        .store
        .get_command(&accepted.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(command
        .receipt
        .envelope
        .authorization
        .authority_id
        .starts_with("nyxid-proxy:"));
    let saved = fixture
        .identity
        .store
        .proxy_grant(&grant_digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.authorization, request.authorization);
    let restarted = proxy_auth(&fixture);
    let proof = restarted
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap();
    assert_eq!(proof.caller().authorization, saved.authorization);
    restarted
        .authorize_command_revival(&fixture.access, &command)
        .await
        .unwrap()
        .require_command(&command)
        .unwrap();
    let serialized = serde_json::to_string(&command).unwrap();
    assert!(!serialized.contains(
        headers[crate::auth::nyxid_proxy::jwt::HEADER]
            .to_str()
            .unwrap()
    ));
    assert!(!serialized.contains(OWNER_TOKEN));
    assert!(!ingress.auth.authorize_bot_request(&request).await.unwrap());
}

#[tokio::test]
async fn invalid_signed_ingress_never_falls_back_to_an_independent_owner_bearer() {
    use crate::auth::nyxid_proxy::jwt::{Signer, HEADER};
    let fixture = ResourceFixture::with_owner("owner").await;
    let ingress = SignedIngress::new(&fixture).await;
    for failure in [
        "signature",
        "issuer",
        "audience",
        "service",
        "expired",
        "duplicate",
    ] {
        let mut claims = signed_claims();
        match failure {
            "issuer" => claims["iss"] = json!("https://another-issuer.example"),
            "audience" => claims["aud"] = json!("https://another-api.example"),
            "service" => claims["nyx_service_id"] = json!("another-service"),
            "expired" => {
                claims["iat"] = json!(1);
                claims["exp"] = json!(61);
            }
            _ => {}
        }
        let mut headers = signed_headers(&claims);
        if failure == "signature" {
            headers.insert(
                HEADER,
                Signer::rotated()
                    .token(&claims, "proxy-test-key")
                    .parse()
                    .unwrap(),
            );
        }
        if failure == "duplicate" {
            headers.append(HEADER, headers[HEADER].clone());
        }
        // This bearer would independently resolve through the provider fixture.
        // Its presence cannot rescue an explicit invalid proxy assertion.
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {OWNER_TOKEN}").parse().unwrap(),
        );
        assert!(
            ingress
                .auth
                .resolve_request(&headers)
                .await
                .unwrap()
                .is_none(),
            "{failure}"
        );
    }
}

#[tokio::test]
async fn signed_foreign_subject_and_missing_permissions_cannot_acquire_owner_write_authority() {
    let fixture = ResourceFixture::with_owner("owner").await;
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:*"]}));
    let ingress = SignedIngress::new(&fixture).await;
    for failure in ["foreign_subject", "no_permissions"] {
        let mut claims = signed_claims();
        if failure == "foreign_subject" {
            claims["sub"] = json!("another-owner");
        } else {
            claims["permissions"] = json!([]);
        }
        let request = ingress
            .auth
            .resolve_request(&signed_headers(&claims))
            .await
            .unwrap()
            .unwrap();
        if failure == "foreign_subject" {
            assert_eq!(request.subject, SubjectId::new("another-owner"));
            assert!(request.delegated_credential.is_none());
        } else {
            assert_eq!(request.authorization, Authorization::none());
        }
        let context = ingress.auth.resource_request_context(&request);
        assert!(
            service(&fixture, ingress.auth.clone())
                .admit(
                    &context,
                    request.delegated_credential.as_deref().unwrap_or(""),
                    CommandSubmissionSource::Api,
                    submission(failure),
                )
                .await
                .is_err(),
            "{failure}"
        );
    }
}

#[tokio::test]
async fn browser_domain_migration_preserves_only_the_explicit_proxy_audience() {
    let fixture = ResourceFixture::with_owner("owner").await;
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:*"]}));
    let ingress = SignedIngress::with_audience(
        &fixture,
        Some("https://new-cma.example.test"),
        Some("https://cma.example.test".into()),
    )
    .await;
    let claims = signed_claims();
    assert!(ingress
        .auth
        .resolve_request(&signed_headers(&claims))
        .await
        .unwrap()
        .is_some());
    for rejected in [
        "https://new-cma.example.test",
        "https://foreign.example",
        "https://cma.example.test/",
    ] {
        let mut claims = claims.clone();
        claims["aud"] = json!(rejected);
        assert!(
            ingress
                .auth
                .resolve_request(&signed_headers(&claims))
                .await
                .unwrap()
                .is_none(),
            "{rejected}"
        );
    }
}
