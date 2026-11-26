//! Deterministic CMA auth composition; only the external NyxID config is doubled.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::{to_bytes, Body};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use layer2_core::{Authorization, Capability, SubjectId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::persistence::{
    AuthStore, BrowserLogin, DurableSubjectTokens, LoginAttempt, MemoryAuthStore, SecretDigest,
    SecretValue, SubjectAuthState,
};
use super::{AuthMode, AuthState};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    version: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    operation: String,
    scenario: String,
    request: RequestFixture,
    response: ResponseFixture,
    #[serde(default)]
    captures: BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestFixture {
    method: String,
    path: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    #[serde(default, rename = "rawBody", skip_serializing_if = "Option::is_none")]
    raw_body: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResponseFixture {
    status: u16,
    #[serde(default, rename = "setCookies", skip_serializing_if = "Vec::is_empty")]
    set_cookies: Vec<String>,
    headers: BTreeMap<String, String>,
    body: Value,
    #[serde(rename = "bodyKind")]
    body_kind: String,
}

struct Scenario {
    router: Router,
    upstream: tokio::task::JoinHandle<()>,
    config_requests: Arc<AtomicUsize>,
}

impl Drop for Scenario {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}

async fn scenario(name: &str) -> Scenario {
    // A `_legal` suffix publishes terms over an otherwise identical scenario.
    let (name, legal) = name
        .strip_suffix("_legal")
        .map_or((name, false), |base| (base, true));
    assert!(
        matches!(
            name,
            "auth_trusted"
                | "auth_anonymous"
                | "auth_anonymous_disabled"
                | "auth_visitor"
                | "auth_expired"
                | "auth_missing_authority"
                | "auth_store_unavailable"
                | "auth_identity_unavailable"
                | "auth_options_available"
                | "auth_options_unavailable"
                | "auth_options_malformed"
                | "auth_options_disabled"
                | "auth_options_ambient_failure"
                | "auth_callback_expired"
                | "auth_callback_cancelled"
                | "auth_callback_silent_declined"
                | "auth_callback_code_missing"
        ),
        "unknown auth scenario {name}"
    );
    let config_requests = Arc::new(AtomicUsize::new(0));
    let observed = config_requests.clone();
    let (status, config) = match name {
        "auth_options_unavailable" => (StatusCode::BAD_GATEWAY, json!({})),
        "auth_options_malformed" => (StatusCode::OK, json!({"social_providers": "invalid"})),
        "auth_options_disabled" => (
            StatusCode::OK,
            json!({
                "social_providers": [], "email_auth_enabled": false, "invite_code_required": false,
            }),
        ),
        _ => (
            StatusCode::OK,
            json!({
                "social_providers": ["github", "google"], "email_auth_enabled": true,
                "invite_code_required": false, "frontend_url": "https://identity.example.test",
                "deployment_internal": "must-not-cross-cma-boundary",
            }),
        ),
    };
    let identity_unavailable = name == "auth_identity_unavailable";
    let upstream_router = Router::new().route("/userinfo", get(move || async move {
        (if identity_unavailable { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK }, Json(json!({
            "sub": "contract-visitor", "name": "Contract Visitor", "email": "visitor@example.test",
            "permissions": ["cma:sandbox:use", "cma:sandbox:delete", "cma:trigger:launch"],
        })))
    })).route(
        "/api/v1/public/config",
        get(move || {
            let observed = observed.clone();
            let config = config.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                (status, Json(config))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let upstream =
        tokio::spawn(async move { axum::serve(listener, upstream_router).await.unwrap() });
    let store = Arc::new(MemoryAuthStore::default());
    if matches!(
        name,
        "auth_visitor" | "auth_expired" | "auth_missing_authority" | "auth_identity_unavailable"
    ) {
        let subject = SubjectId::new("contract-visitor");
        store
            .put_browser_login(BrowserLogin {
                id: SecretDigest::identify("browser-login", "contract-browser"),
                subject: subject.clone(),
                expires_at: if name == "auth_expired" {
                    SystemTime::UNIX_EPOCH
                } else {
                    SystemTime::now() + Duration::from_secs(3600)
                },
            })
            .await
            .unwrap();
        if name != "auth_missing_authority" {
            store
                .put_subject(SubjectAuthState::new(
                    subject,
                    Authorization::new([
                        Capability::UseOwnSandboxes,
                        Capability::DeleteOwnSandbox,
                        Capability::LaunchTriggers,
                    ]),
                    super::profile::Profile {
                        display_name: Some("Contract Visitor".into()),
                        email: Some("visitor@example.test".into()),
                        avatar_url: None,
                    },
                    Some(DurableSubjectTokens {
                        access_token: SecretValue::new("contract-durable-access".into()),
                        refresh_token: Some(SecretValue::new("contract-durable-refresh".into())),
                        expires_at: SystemTime::now() + Duration::from_secs(3600),
                    }),
                ))
                .await
                .unwrap();
        }
    }
    if matches!(
        name,
        "auth_callback_expired"
            | "auth_callback_cancelled"
            | "auth_callback_code_missing"
            | "auth_callback_silent_declined"
    ) {
        store
            .put_login_attempt(LoginAttempt {
                id: SecretDigest::identify("oidc-login-attempt", "contract-state"),
                browser_binding: SecretDigest::identify("oidc-browser-binding", "contract-binding"),
                verifier: SecretValue::new("contract-verifier".into()),
                return_to: Some("/t/public-contract/placement-contract".into()),
                actor_link_intent: None,
                service_authorization: None,
                silent: name == "auth_callback_silent_declined",
                consent_requested: false,
                custody_reserved: false,
                started_at: None,
                expires_at: if name == "auth_callback_expired" {
                    SystemTime::UNIX_EPOCH
                } else {
                    SystemTime::now() + Duration::from_secs(3600)
                },
            })
            .await
            .unwrap();
    }
    if name == "auth_anonymous" {
        let binding = SecretDigest::identify("browser-login", "contract-preference-binding");
        store
            .reserve_browser_login(
                binding.clone(),
                SystemTime::now() + Duration::from_secs(600),
            )
            .await
            .unwrap();
        store
            .complete_browser_login(
                &binding,
                BrowserLogin {
                    id: SecretDigest::identify("browser-login", "contract-preference-session"),
                    subject: SubjectId::new("contract-visitor"),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                },
                Some(SystemTime::UNIX_EPOCH + Duration::from_millis(1001)),
            )
            .await
            .unwrap();
    }
    store.set_unavailable(matches!(
        name,
        "auth_store_unavailable" | "auth_options_ambient_failure"
    ));
    let mode = if name == "auth_trusted" {
        AuthMode::Trusted {
            subject: "contract-developer".into(),
        }
    } else {
        AuthMode::Provider {
            base_url: base_url.clone(),
            client_id: "contract-client".into(),
            client_secret: "contract-value".into(),
            public_url: "https://cma.example.test".into(),
        }
    };
    let mut auth = AuthState::with_store_and_nyxid_public_url(
        mode,
        None,
        store,
        Some("https://identity.example.test".into()),
    )
    .with_silent_sign_in(name != "auth_anonymous_disabled")
    .with_legal_terms(legal.then(|| {
        layer2_core::legal_consent::LegalTerms::new(
            "2026-11-30",
            "https://legal.example.test/terms",
            "https://legal.example.test/privacy",
        )
        .unwrap()
    }));
    Arc::get_mut(&mut auth).unwrap().preference_clock =
        || SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    if matches!(
        name,
        "auth_visitor" | "auth_store_unavailable" | "auth_identity_unavailable"
    ) {
        *auth.metadata.write().await = Some(super::ProviderMetadata {
            authorization_endpoint: format!("{base_url}/authorize"),
            token_endpoint: format!("{base_url}/token"),
            userinfo_endpoint: Some(format!("{base_url}/userinfo")),
            introspection_endpoint: None,
        });
    }
    Scenario {
        router: super::flow::router(auth.clone()).layer(axum::middleware::from_fn_with_state(
            auth,
            super::attach_subject,
        )),
        upstream,
        config_requests,
    }
}

#[tokio::test]
async fn shared_auth_contract_matches_real_cma_composition() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../layer1-public/tests/fixtures/public-contract");
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|value| value == "json")
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("auth-")
                // Delegated resource operations use the complete broker fixture.
                && path.file_name().unwrap() != "auth-agent-delegations.json"
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "CMA auth corpus must exist");
    let record = ["CMA_RECORD_PUBLIC_CONTRACT", "CMA_UPDATE_PUBLIC_CONTRACT"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| value == "1"));
    let selected = std::env::var("CMA_PUBLIC_CONTRACT_FILE").ok();
    let mut total = 0;
    for path in paths {
        if selected
            .as_ref()
            .is_some_and(|name| path.file_name().unwrap() != name.as_str())
        {
            continue;
        }
        let mut corpus: Corpus = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(corpus.version, 1);
        let mut scenarios = BTreeMap::new();
        for case in &mut corpus.cases {
            assert!(
                case.captures.is_empty(),
                "auth fixtures use exact nonsecret values, not normalized authorities"
            );
            assert_eq!(
                case.operation,
                format!(
                    "{} {}",
                    case.request.method,
                    case.request.path.split('?').next().unwrap()
                )
            );
            if !scenarios.contains_key(&case.scenario) {
                scenarios.insert(case.scenario.clone(), scenario(&case.scenario).await);
            }
            replay(scenarios.get(&case.scenario).unwrap(), case, record).await;
            total += 1;
        }
        for (name, scenario) in &scenarios {
            if name.starts_with("auth_options_") {
                assert_eq!(
                    scenario.config_requests.load(Ordering::SeqCst),
                    1,
                    "{name} config is fetched once and cached"
                );
            }
        }
        if record {
            std::fs::write(
                path,
                format!("{}\n", serde_json::to_string_pretty(&corpus).unwrap()),
            )
            .unwrap();
        }
    }
    assert!(total > 0, "no auth contract cases selected");
    eprintln!("Replayed {total} shared auth cases against real CMA handlers; external NyxID public-config boundary doubled");
}

async fn replay(scenario: &Scenario, case: &mut Case, record: bool) {
    let mut request = Request::builder()
        .method(case.request.method.as_str())
        .uri(&case.request.path);
    for (name, value) in &case.request.headers {
        request = request.header(name, value);
    }
    assert!(case.request.body.is_none() || case.request.raw_body.is_none());
    let body = case
        .request
        .raw_body
        .clone()
        .or_else(|| case.request.body.as_ref().map(Value::to_string));
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        scenario.router.clone().oneshot(
            request
                .body(body.map_or_else(Body::empty, Body::from))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        response.status().as_u16(),
        case.response.status,
        "{} status",
        case.id
    );
    let set_cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let headers = response_headers(response.headers());
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body: Value = match case.response.body_kind.as_str() {
        "json" => serde_json::from_slice(&bytes).unwrap(),
        "empty" => {
            assert!(bytes.is_empty(), "{} empty body", case.id);
            Value::Null
        }
        "text" => Value::String(String::from_utf8(bytes.to_vec()).unwrap()),
        other => panic!("unsupported auth body kind {other}"),
    };
    assert!(!body.to_string().contains("must-not-cross-cma-boundary"));
    if record {
        case.response.headers = headers;
        case.response.set_cookies = set_cookies;
        case.response.body = body;
    } else {
        assert_eq!(headers, case.response.headers, "{} headers", case.id);
        assert_eq!(
            set_cookies, case.response.set_cookies,
            "{} Set-Cookie values",
            case.id
        );
        assert_eq!(body, case.response.body, "{} body", case.id);
    }
}

fn response_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (name, value) in headers {
        if name == "set-cookie" {
            continue;
        }
        assert!(
            values
                .insert(name.as_str().to_owned(), value.to_str().unwrap().to_owned())
                .is_none(),
            "single-value header fixtures cannot represent repeated {name}"
        );
    }
    values.remove("content-length");
    values
}

#[test]
fn repeated_response_headers_cannot_hide_earlier_values() {
    for name in ["cache-control", "location"] {
        let mut headers = HeaderMap::new();
        headers.append(name, "unexpected=earlier".parse().unwrap());
        headers.append(name, "expected=last".parse().unwrap());
        assert!(std::panic::catch_unwind(|| response_headers(&headers)).is_err());
    }
}

#[test]
fn single_value_response_headers_keep_exact_values_except_derived_length() {
    let mut headers = HeaderMap::new();
    headers.insert("location", "/expected".parse().unwrap());
    headers.insert("content-length", "0".parse().unwrap());
    assert_eq!(
        response_headers(&headers),
        BTreeMap::from([("location".into(), "/expected".into())])
    );
}

#[test]
fn repeated_cookie_headers_are_kept_separately_without_comma_folding() {
    let mut headers = HeaderMap::new();
    headers.append(
        "set-cookie",
        "first=1; Expires=Wed, 11 Nov 2026 12:00:00 GMT"
            .parse()
            .unwrap(),
    );
    headers.append("set-cookie", "second=2; HttpOnly".parse().unwrap());
    assert!(response_headers(&headers).is_empty());
    assert_eq!(
        headers
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "first=1; Expires=Wed, 11 Nov 2026 12:00:00 GMT",
            "second=2; HttpOnly"
        ]
    );
}
