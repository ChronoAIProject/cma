use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use serde_json::json;

use super::*;
#[path = "agent_skills_tests/authority_tests.rs"]
mod authority_tests;
#[path = "agent_skills_tests/warmup_tests.rs"]
mod warmup_tests;
use authority_tests::{sandbox, CredentialReply, TestCredentials};

const TOKEN: &str = "admitted-sandbox-key";
const LOCATOR_ORIGIN: &str = "https://platform.example";

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn json(value: serde_json::Value) -> Self {
        Self {
            status: StatusCode::OK,
            body: serde_json::to_vec(&value).unwrap(),
        }
    }

    fn status(status: StatusCode) -> Self {
        Self {
            status,
            body: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenRequest {
    target: String,
    authorization: Option<String>,
}

#[derive(Clone, Default)]
struct RegistryState {
    replies: Arc<Mutex<BTreeMap<String, VecDeque<Reply>>>>,
    requests: Arc<Mutex<Vec<SeenRequest>>>,
}

struct Registry {
    origin: String,
    state: RegistryState,
    task: tokio::task::JoinHandle<()>,
}

impl Registry {
    async fn start() -> Self {
        let state = RegistryState::default();
        let app = Router::new()
            .fallback(registry_reply)
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            origin: format!("http://{address}"),
            state,
            task,
        }
    }

    fn reply(&self, id: &str, version: &str, reply: Reply) {
        let target = detail_target(id, version);
        self.state
            .replies
            .lock()
            .unwrap()
            .entry(target)
            .or_default()
            .push_back(reply);
    }

    fn requests(&self) -> Vec<SeenRequest> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn registry_reply(
    State(state): State<RegistryState>,
    request: Request<Body>,
) -> Response<Body> {
    let target = request
        .uri()
        .path_and_query()
        .map(ToString::to_string)
        .unwrap_or_else(|| request.uri().path().to_owned());
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    state.requests.lock().unwrap().push(SeenRequest {
        target: target.clone(),
        authorization,
    });
    let reply = state
        .replies
        .lock()
        .unwrap()
        .get_mut(&target)
        .and_then(VecDeque::pop_front)
        .unwrap_or_else(|| Reply::status(StatusCode::NOT_FOUND));
    Response::builder()
        .status(reply.status)
        .body(Body::from(reply.body))
        .unwrap()
}

fn resolver(
    registry: &Registry,
    credentials: Option<Arc<TestCredentials>>,
) -> OrnnAgentSkillResolver {
    OrnnAgentSkillResolver {
        warmup_tokens: None,
        http: reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        request_origin: reqwest::Url::parse(&registry.origin).unwrap(),
        locator_origin: reqwest::Url::parse(LOCATOR_ORIGIN).unwrap(),
        credentials: credentials.map(|value| value as Arc<dyn SandboxCredentialController>),
    }
}

fn reference(id: &str, version: Option<&str>) -> SkillRef {
    SkillRef {
        source: SkillSource::Ornn,
        id: layer2_core::SkillId::new(id),
        version: version.map(str::to_owned),
    }
}

fn material(id: &str, version: &str, hash: &str) -> serde_json::Value {
    json!({
        "data": {
            "guid": id,
            "name": id,
            "version": version,
            "skillHash": hash,
            "metadata": {"ignored": true}
        },
        "error": null
    })
}

fn detail_target(id: &str, version: &str) -> String {
    let origin = reqwest::Url::parse("http://fixture.invalid").unwrap();
    let url = skill_detail_url(&origin, id, (!version.is_empty()).then_some(version)).unwrap();
    url.query().map_or_else(
        || url.path().to_owned(),
        |query| format!("{}?{query}", url.path()),
    )
}

#[tokio::test]
async fn empty_resolution_needs_neither_owner_token_nor_network() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::with(CredentialReply::Failed("must not be called".into()));
    let resolved = resolver(&registry, Some(Arc::clone(&credentials)))
        .resolve(&sandbox(), &[])
        .await
        .unwrap();

    assert!(resolved.is_empty());
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 0);
    assert!(registry.requests().is_empty());
}

#[tokio::test]
async fn exact_pinned_material_is_authenticated_and_keeps_reference_order() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_alpha",
        "1.2",
        Reply::json(material("skl_alpha", "1.2", &"a".repeat(64))),
    );
    registry.reply(
        "skl_beta",
        "3.4",
        Reply::json(material("skl_beta", "3.4", &"b".repeat(64))),
    );
    let credentials = TestCredentials::present();
    let references = [
        reference("skl_beta", Some("3.4")),
        reference("skl_alpha", Some("1.2")),
    ];

    let resolved = resolver(&registry, Some(Arc::clone(&credentials)))
        .resolve(&sandbox(), &references)
        .await
        .unwrap();

    assert_eq!(
        resolved
            .iter()
            .map(|skill| skill.id.as_str())
            .collect::<Vec<_>>(),
        ["skl_beta", "skl_alpha"]
    );
    assert_eq!(resolved[0].version, "3.4");
    assert_eq!(resolved[0].archive_sha256, "b".repeat(64));
    assert_eq!(
        resolver(&registry, None).runtime_skills(&resolved).unwrap()[0].source_url,
        concat!(
            "https://platform.example/api/v1/proxy/s/ornn-api/api/v1/skills/",
            "skl_beta/versions/3.4/download"
        )
    );
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 1);
    let requests = registry.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.authorization.as_deref() == Some("Bearer admitted-sandbox-key")));
}

#[tokio::test]
async fn duplicate_references_fail_before_authority_is_read() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::present();
    let references = [
        reference("skl_repeat", Some("2.0")),
        reference("skl_repeat", Some("2.0")),
    ];

    let error = resolver(&registry, Some(Arc::clone(&credentials)))
        .resolve(&sandbox(), &references)
        .await
        .unwrap_err();

    assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 0);
    assert!(registry.requests().is_empty());
}

#[tokio::test]
async fn invalid_version_grammar_fails_before_authority_is_read() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::present();
    for version in [
        Some("latest"),
        Some("1.2.3"),
        Some("01.2"),
        Some("@"),
        Some("@Latest"),
        Some("@with.dot"),
        Some("@1stable"),
    ] {
        let error = resolver(&registry, Some(Arc::clone(&credentials)))
            .resolve(&sandbox(), &[reference("skl_a", version)])
            .await
            .unwrap_err();
        assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
    }
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 0);
    assert!(registry.requests().is_empty());
}

#[tokio::test]
async fn omitted_and_real_dist_tags_resolve_once_to_literal_material() {
    for version in [
        None,
        Some(""),
        Some("@latest"),
        Some("@stable"),
        Some("@rc-1"),
    ] {
        let registry = Registry::start().await;
        registry.reply(
            "skl_alias",
            version.unwrap_or(""),
            Reply::json(material("skl_alias", "3.4", &"a".repeat(64))),
        );
        let credentials = TestCredentials::present();
        let resolver = resolver(&registry, Some(Arc::clone(&credentials)));
        let references = [reference("skl_alias", version)];
        let resolved = resolver.resolve(&sandbox(), &references).await.unwrap();
        assert_eq!(resolved[0].version, "3.4");
        assert_eq!(references[0].version.as_deref(), version);
        validate_skill_selection(&references, &resolved).unwrap();
        let first = resolver.runtime_skills(&resolved).unwrap();
        let second = resolver.runtime_skills(&resolved).unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].version.as_deref(), Some("3.4"));
        assert!(first[0].source_url.ends_with("/versions/3.4/download"));
        assert_eq!(credentials.calls.load(Ordering::Relaxed), 1);
        assert_eq!(registry.requests().len(), 1);
    }
}

#[tokio::test]
async fn persisted_material_rebuilds_current_locators_without_metadata_or_tokens() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::with(CredentialReply::Failed("must not be called".into()));
    let mut resolver = resolver(&registry, Some(Arc::clone(&credentials)));
    let material = vec![ResolvedAgentSkill {
        source: SkillSource::Ornn,
        id: layer2_core::SkillId::new("skl_persisted"),
        name: None,
        version: "2.10".into(),
        archive_sha256: "b".repeat(64),
    }];
    let first = resolver.runtime_skills(&material).unwrap();
    resolver.locator_origin = reqwest::Url::parse("https://replacement.example").unwrap();
    let second = resolver.runtime_skills(&material).unwrap();
    assert_ne!(first[0].source_url, second[0].source_url);
    assert_eq!(first[0].id, second[0].id);
    assert_eq!(first[0].version, second[0].version);
    assert_eq!(first[0].archive_sha256, second[0].archive_sha256);
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 0);
    assert!(registry.requests().is_empty());
    for invalid in ["2.10.0", "@latest", "02.10"] {
        let mut changed = material.clone();
        changed[0].version = invalid.into();
        assert!(matches!(
            resolver.runtime_skills(&changed),
            Err(AgentSkillResolutionError::Invalid(_))
        ));
    }
}

#[tokio::test]
async fn inaccessible_or_absent_exact_reference_is_not_found() {
    for status in [StatusCode::FORBIDDEN, StatusCode::NOT_FOUND] {
        let registry = Registry::start().await;
        registry.reply("skl_missing", "1.0", Reply::status(status));
        let error = resolver(&registry, Some(TestCredentials::present()))
            .resolve(&sandbox(), &[reference("skl_missing", Some("1.0"))])
            .await
            .unwrap_err();
        assert!(matches!(error, AgentSkillResolutionError::NotFound(_)));
    }
}

#[tokio::test]
async fn transient_http_failures_are_retryable() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let registry = Registry::start().await;
        registry.reply("skl_busy", "1.0", Reply::status(status));
        let error = resolver(&registry, Some(TestCredentials::present()))
            .resolve(&sandbox(), &[reference("skl_busy", Some("1.0"))])
            .await
            .unwrap_err();
        assert!(matches!(error, AgentSkillResolutionError::Unavailable(_)));
    }
}

#[tokio::test]
async fn identity_version_and_digest_mismatches_are_invalid() {
    let cases = [
        material("skl_other", "1.0", &"a".repeat(64)),
        material("skl_exact", "2.0", &"a".repeat(64)),
        material("skl_exact", "1.0", &"A".repeat(64)),
        material("skl_exact", "1.0", "sha256:not-canonical"),
        json!({"data": null, "error": null}),
        json!({
            "data": {
                "guid": "skl_exact",
                "version": "1.0",
                "skillHash": "a".repeat(64)
            },
            "error": {"code": "secret-upstream-error"}
        }),
    ];
    for response in cases {
        let registry = Registry::start().await;
        registry.reply("skl_exact", "1.0", Reply::json(response));
        let error = resolver(&registry, Some(TestCredentials::present()))
            .resolve(&sandbox(), &[reference("skl_exact", Some("1.0"))])
            .await
            .unwrap_err();
        assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
    }
}

#[tokio::test]
async fn malformed_and_oversized_bodies_are_invalid_without_body_leakage() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_bad_json",
        "1.0",
        Reply {
            status: StatusCode::OK,
            body: b"not-json PRIVATE_RESPONSE_BODY".to_vec(),
        },
    );
    registry.reply(
        "skl_large",
        "1.0",
        Reply {
            status: StatusCode::OK,
            body: vec![b'x'; MAX_RESPONSE_BYTES + 1],
        },
    );

    for id in ["skl_bad_json", "skl_large"] {
        let error = resolver(&registry, Some(TestCredentials::present()))
            .resolve(&sandbox(), &[reference(id, Some("1.0"))])
            .await
            .unwrap_err();
        let display = error.to_string();
        assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
        assert!(!display.contains("PRIVATE_RESPONSE_BODY"));
        assert!(!display.contains(&"x".repeat(100)));
    }
}

#[tokio::test]
async fn credential_failures_do_not_reveal_token_diagnostics() {
    let registry = Registry::start().await;
    for reply in [
        CredentialReply::Missing,
        CredentialReply::Failed("credential lookup echoed SUPER_SECRET_TOKEN".into()),
    ] {
        let error = resolver(&registry, Some(TestCredentials::with(reply)))
            .resolve(&sandbox(), &[reference("skl_private", Some("1.0"))])
            .await
            .unwrap_err();
        let display = error.to_string();
        assert!(matches!(error, AgentSkillResolutionError::Unavailable(_)));
        assert!(!display.contains("SUPER_SECRET_TOKEN"));
        assert!(!display.contains(TOKEN));
    }
    assert!(registry.requests().is_empty());
}

#[tokio::test]
async fn one_failed_reference_returns_no_partial_collection() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_ok",
        "1.0",
        Reply::json(material("skl_ok", "1.0", &"d".repeat(64))),
    );
    registry.reply("skl_missing", "1.0", Reply::status(StatusCode::NOT_FOUND));
    let references = [
        reference("skl_ok", Some("1.0")),
        reference("skl_missing", Some("1.0")),
    ];

    let result = resolver(&registry, Some(TestCredentials::present()))
        .resolve(&sandbox(), &references)
        .await;

    assert!(matches!(
        result,
        Err(AgentSkillResolutionError::NotFound(_))
    ));
}

#[tokio::test]
async fn anonymous_mode_resolves_public_material_without_authorization() {
    let registry = Registry::start().await;
    registry.reply(
        "skl_public",
        "1.0",
        Reply::json(material("skl_public", "1.0", &"e".repeat(64))),
    );

    let resolved = resolver(&registry, None)
        .resolve(&sandbox(), &[reference("skl_public", Some("1.0"))])
        .await
        .unwrap();

    assert_eq!(resolved.len(), 1);
    assert_eq!(registry.requests()[0].authorization, None);
}

#[tokio::test]
async fn selected_baseline_skills_resolve_and_retain_exact_archive_pins() {
    let registry = Registry::start().await;
    let ids = ["nyxid", "ornn-agent-manual-cli"];
    for id in ids {
        registry.reply(id, "", Reply::json(material(id, "2.0", &"b".repeat(64))));
    }
    let resolver = resolver(&registry, Some(TestCredentials::present()));
    let references = ids.map(|id| reference(id, None));
    let resolved = resolver.resolve(&sandbox(), &references).await.unwrap();
    validate_skill_selection(&references, &resolved).unwrap();
    let runtime = resolver.runtime_skills(&resolved).unwrap();
    for (id, skill) in ids.iter().zip(runtime) {
        assert_eq!(skill.id, *id);
        assert_eq!(skill.name.as_deref(), Some(*id));
        assert_eq!(skill.version.as_deref(), Some("2.0"));
        assert_eq!(skill.archive_sha256, "b".repeat(64));
        assert!(skill
            .source_url
            .ends_with(&format!("/{id}/versions/2.0/download")));
    }
}

#[tokio::test]
async fn malformed_runtime_ids_fail_before_authority_is_read() {
    let registry = Registry::start().await;
    let credentials = TestCredentials::present();
    let too_long = "x".repeat(65);
    for id in [
        "../escape",
        ".hidden",
        "has.dot",
        "has/slash",
        "has space",
        too_long.as_str(),
    ] {
        let error = resolver(&registry, Some(Arc::clone(&credentials)))
            .resolve(&sandbox(), &[reference(id, Some("1.0"))])
            .await
            .unwrap_err();
        assert!(matches!(error, AgentSkillResolutionError::Invalid(_)));
    }
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 0);
    assert!(registry.requests().is_empty());
}

#[test]
fn production_constructor_requires_a_canonical_https_origin() {
    for invalid in [
        "http://platform.example",
        "https://platform.example/",
        "https://user@platform.example",
        "https://platform.example/path",
        "https://platform.example?query=1",
    ] {
        assert!(
            OrnnAgentSkillResolver::anonymous(invalid).is_err(),
            "{invalid}"
        );
    }
    assert!(OrnnAgentSkillResolver::anonymous(LOCATOR_ORIGIN).is_ok());
}
