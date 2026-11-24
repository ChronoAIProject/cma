use super::*;
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::SandboxPolicy;
use layer2_core::{Authorization, SubjectId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[path = "agent_access_tests/tenant_isolation.rs"]
mod tenant_isolation;

struct Authority {
    revoked: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl PersonalCommandAdmissionAuthority for Authority {
    async fn personal_command_authority(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.revoked.load(Ordering::SeqCst)
            || caller.subject.as_str() != "owner"
            || credential != "owner-fixture-credential"
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: "personal:fixture-owner-login".into(),
            policy_revision: 7,
            grant_revision: None,
        })
    }
}

// Resolve an actual retained browser login through the request authentication
// path so this fixture follows its attribution instead of assuming Human.
struct BrowserCookie {
    auth: Arc<crate::auth::AuthState>,
    context: CallerContext,
    allowed: Arc<AtomicBool>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for BrowserCookie {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl BrowserCookie {
    async fn new() -> Self {
        use crate::auth::persistence::{
            AuthStore, BrowserLogin, DurableSubjectTokens, MemoryAuthStore, SecretDigest,
            SecretValue, SubjectAuthState,
        };
        use crate::auth::{AuthMode, AuthState, ProviderMetadata, AUTH_COOKIE};
        use axum::http::{header, HeaderMap, HeaderValue};
        use axum::routing::{get, post};
        use axum::{Json, Router};
        use serde_json::json;
        use std::time::{Duration, SystemTime};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let allowed = Arc::new(AtomicBool::new(true));
        let permission = allowed.clone();
        let router = Router::new()
            .route(
                "/userinfo",
                get(|| async { Json(json!({"sub":"owner", "permissions":["cma:sandbox:use"]})) }),
            )
            .route(
                "/introspect",
                post(move || {
                    let permission = permission.clone();
                    async move {
                        let permissions = if permission.load(Ordering::SeqCst) {
                            vec!["cma:sandbox:use"]
                        } else {
                            vec![]
                        };
                        Json(json!({"active":true,"sub":"owner","permissions":permissions}))
                    }
                }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let store = Arc::new(MemoryAuthStore::default());
        let expires_at = SystemTime::now() + Duration::from_secs(3600);
        store
            .put_subject(SubjectAuthState::new(
                SubjectId::new("owner"),
                Authorization::new([Capability::UseOwnSandboxes]),
                Default::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new("owner-fixture-credential".into()),
                    refresh_token: None,
                    expires_at,
                }),
            ))
            .await
            .unwrap();
        store
            .put_browser_login(BrowserLogin {
                id: SecretDigest::identify("browser-login", "owner-fixture-cookie"),
                subject: SubjectId::new("owner"),
                expires_at,
            })
            .await
            .unwrap();
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "fixture-client".into(),
                client_secret: "fixture-secret".into(),
                public_url: "https://cma.example.test".into(),
            },
            None,
            store,
        );
        *auth.fixture_metadata().write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            introspection_endpoint: Some(format!("{base}/introspect")),
        });
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{AUTH_COOKIE}=owner-fixture-cookie")).unwrap(),
        );
        let request = auth.resolve_request(&headers).await.unwrap().unwrap();
        assert_eq!(
            request.delegated_credential.as_deref(),
            Some("owner-fixture-credential")
        );
        let context = auth.resource_request_context(&request);
        assert_eq!(context.credential_source, CredentialSource::Retained);
        assert_eq!(
            context.actor,
            ResourceActor::Api {
                subject: SubjectId::new("owner"),
                delegation_id: None,
            }
        );
        assert!(context.validate_principal());
        Self {
            auth,
            context,
            allowed,
            server,
        }
    }
}

fn ineligible_contexts(context: &CallerContext) -> Vec<CallerContext> {
    (0..10)
        .map(|case| {
            let mut denied = context.clone();
            match case {
                0 => denied.credential_source = CredentialSource::Independent { expires_at: None },
                1 => denied.credential_source = CredentialSource::Binding,
                2 | 3 => {
                    denied.actor = ResourceActor::Api {
                        subject: context.caller.subject.clone(),
                        delegation_id: Some("grant".into()),
                    };
                    if case == 3 {
                        denied.credential_source = CredentialSource::delegated("grant");
                    }
                }
                4 => denied.credential.subject = SubjectId::new("foreign"),
                5 => denied.caller.authorization = Authorization::none(),
                6 => {
                    denied.actor = ResourceActor::Api {
                        subject: SubjectId::new("foreign"),
                        delegation_id: None,
                    }
                }
                7 => {
                    denied.actor = ResourceActor::Human {
                        subject: SubjectId::new("foreign"),
                    }
                }
                8 => denied.credential.credential_id.clear(),
                _ => {
                    denied.actor = ResourceActor::Trigger {
                        subject: context.caller.subject.clone(),
                        launch_id: layer2_core::LaunchId("launch-fixture".into()),
                    }
                }
            }
            denied
        })
        .collect()
}

#[tokio::test]
async fn browser_cookie_approval_requires_exact_durable_token_and_current_personal_gate() {
    let browser = BrowserCookie::new().await;
    let admitted = retained_owner_authority(
        &browser.context,
        "owner-fixture-credential",
        browser.auth.as_ref(),
    )
    .await
    .unwrap();
    assert!(admitted.authority_id.starts_with("personal:"));
    let mut human = browser.context.clone();
    human.actor = ResourceActor::Human {
        subject: human.caller.subject.clone(),
    };
    assert_eq!(
        retained_owner_authority(&human, "owner-fixture-credential", browser.auth.as_ref(),)
            .await
            .unwrap(),
        admitted
    );
    assert_eq!(
        retained_owner_authority(
            &browser.context,
            "independent-or-runtime-credential",
            browser.auth.as_ref(),
        )
        .await,
        Err(AgentAccessError::Forbidden)
    );
    browser.allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        retained_owner_authority(
            &browser.context,
            "owner-fixture-credential",
            browser.auth.as_ref(),
        )
        .await,
        Err(AgentAccessError::Forbidden)
    );
}

#[tokio::test]
async fn approval_rejects_other_custody_and_invalid_principals_before_authority_lookup() {
    let browser = BrowserCookie::new().await;
    let authority = Authority {
        revoked: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    };
    for denied in ineligible_contexts(&browser.context) {
        assert_eq!(
            retained_owner_authority(&denied, "owner-fixture-credential", &authority,).await,
            Err(AgentAccessError::Forbidden)
        );
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_service_requires_current_retained_owner_authority_and_stores_no_credential() {
    let (client, name, store, first) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let authority = Arc::new(Authority {
        revoked: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    let service = AgentAccessService::new(
        store.clone(),
        authority.clone(),
        Arc::new(SandboxPolicy {
            baseline_services: vec![ServiceId::new("baseline")],
            ..Default::default()
        }),
    );
    let mut proposal = first.proposal.clone();
    proposal.request_key = "human-consent".into();
    let record = store
        .request_agent_access(first.requester.clone(), proposal)
        .await
        .unwrap();
    let browser = BrowserCookie::new().await;
    let context = browser.context.clone();
    assert_eq!(context.caller.subject, record.requester.owner);
    let services = vec!["baseline".into(), "github".into()];
    for denied in ineligible_contexts(&context) {
        assert_eq!(
            service
                .approve(
                    &denied,
                    "owner-fixture-credential",
                    &record.requester.agent_id,
                    &record.id,
                    services.clone(),
                    "human-approve"
                )
                .await,
            Err(AgentAccessError::Forbidden)
        );
    }
    assert_eq!(authority.calls.load(Ordering::SeqCst), 0);
    authority.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        service
            .approve(
                &context,
                "owner-fixture-credential",
                &record.requester.agent_id,
                &record.id,
                services.clone(),
                "human-approve"
            )
            .await,
        Err(AgentAccessError::Forbidden)
    );
    authority.revoked.store(false, Ordering::SeqCst);
    assert_eq!(
        service
            .approve(
                &context,
                "independent-or-runtime-credential",
                &record.requester.agent_id,
                &record.id,
                services.clone(),
                "human-approve"
            )
            .await,
        Err(AgentAccessError::Forbidden)
    );
    assert_eq!(
        service
            .approve(
                &context,
                "owner-fixture-credential",
                &record.requester.agent_id,
                &record.id,
                vec!["github".into()],
                "human-approve"
            )
            .await,
        Err(AgentAccessError::Invalid)
    );
    let approved = service
        .approve(
            &context,
            "owner-fixture-credential",
            &record.requester.agent_id,
            &record.id,
            services,
            "human-approve",
        )
        .await
        .unwrap();
    assert_eq!(
        approved.status,
        layer2_core::sandbox::agent_access::AgentAccessStatus::Connecting
    );
    let saved = store
        .read_access_request(&record.id)
        .await
        .unwrap()
        .unwrap();
    let encoded = serde_json::to_string(&saved).unwrap();
    assert!(!encoded.contains("owner-fixture-credential"));
    assert!(!encoded.contains("human-approve"));
    assert!(encoded.contains("personal:fixture-owner-login"));
    client.database(&name).drop().await.unwrap();
}
