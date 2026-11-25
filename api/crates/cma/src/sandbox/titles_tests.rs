use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, HeaderMap as AxumHeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use layer2_core::sandbox::{
    AgentLogKind, AgentTitleGenerationContext, AgentTitleOutcome, AgentTitleService,
    AgentTitleServiceError, AgentTitleSource, InMemoryAgentTitleStore, StoredAgentTitle,
    AGENT_TITLE_FALLBACK_SCALARS, AGENT_TITLE_MAX_SCALARS, AGENT_TITLE_SYSTEM_PROMPT,
};
use layer2_core::AgentId;

use super::*;

#[path = "titles/content_type_tests.rs"]
mod content_type_tests;

const NOW: &str = "2026-09-05T00:00:00Z";
const BEARER: &str = "sandbox-title-bearer";
const DEPLOYMENT_MODEL: &str = "deployment-model";

#[derive(Default)]
struct Capture {
    authorization: Mutex<Option<String>>,
    body: Mutex<Option<serde_json::Value>>,
}

async fn spawn(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the proxy fixture binds");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("the fixture has an address")
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("the proxy fixture serves");
    });
    (origin, task)
}

async fn capture_success(
    State(capture): State<Arc<Capture>>,
    headers: AxumHeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    *capture.authorization.lock().expect("capture lock") = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    *capture.body.lock().expect("capture lock") = Some(body);
    Json(serde_json::json!({
        "output_text": "  ### **\"Durable Proxy Title\"** ###\nignored private line  "
    }))
}

async fn generate_title(
    generator: &HttpAgentTitleGenerator,
    model: &str,
    bearer: &str,
    owner: &str,
    assistant: &str,
) -> (
    Result<AgentTitleOutcome, AgentTitleServiceError>,
    Option<StoredAgentTitle>,
) {
    let id = AgentId("agt_http_title_fixture".into());
    let store = Arc::new(InMemoryAgentTitleStore::default());
    store.register_agent(id.clone()).expect("agent registers");
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: owner.into(),
        },
        AgentLogKind::AgentMessage {
            content: assistant.into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ] {
        store
            .append_agent_log(&id, NOW, kind)
            .expect("turn evidence appends");
    }
    let service = AgentTitleService::new(store, "http-title-holder").expect("service builds");
    let context = AgentTitleGenerationContext::new(model, bearer).expect("context builds");
    let outcome = service
        .generate_agent_title(&id, Some(&context), Some(generator))
        .await;
    let stored = service.title(&id).await.expect("title reads");
    (outcome, stored)
}

#[tokio::test]
async fn sandbox_proxy_uses_deployment_model_prompt_roles_and_scoped_bearer() {
    let capture = Arc::new(Capture::default());
    let (origin, server) = spawn(
        Router::new()
            .route(
                "/sandboxes/runtime-1/model-proxy/v1/responses",
                post(capture_success),
            )
            .with_state(Arc::clone(&capture)),
    )
    .await;
    let generator = HttpAgentTitleGenerator::fixture(
        &format!("{origin}/sandboxes/runtime-1/model-proxy/"),
        DEPLOYMENT_MODEL,
        Duration::from_secs(2),
        MAX_RESPONSE_BYTES,
    );

    let (outcome, stored) = generate_title(
        &generator,
        "deployment-model",
        BEARER,
        "How does proxy title generation work?",
        "It uses the sandbox model proxy.",
    )
    .await;

    assert_eq!(outcome, Ok(AgentTitleOutcome::Generated));
    let stored = stored.expect("generated title is persisted");
    assert_eq!(stored.source, AgentTitleSource::Generated);
    assert_eq!(stored.title.as_str(), "Durable Proxy Title");
    assert!(stored.title.as_str().chars().count() <= AGENT_TITLE_MAX_SCALARS);
    assert_eq!(
        capture
            .authorization
            .lock()
            .expect("capture lock")
            .as_deref(),
        Some("Bearer sandbox-title-bearer")
    );
    let body = capture
        .body
        .lock()
        .expect("capture lock")
        .clone()
        .expect("request was captured");
    assert_eq!(body["model"], "deployment-model");
    assert_eq!(body["instructions"], AGENT_TITLE_SYSTEM_PROMPT);
    assert!(body["instructions"]
        .as_str()
        .expect("instructions are text")
        .contains("3-6 word"));
    assert_eq!(body["stream"], false);
    assert_eq!(body["store"], false);
    assert_eq!(body["max_output_tokens"], MAX_OUTPUT_TOKENS);
    assert_eq!(body["reasoning"]["effort"], "low");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(
        body["input"][0]["content"],
        "How does proxy title generation work?"
    );
    assert_eq!(body["input"][1]["role"], "assistant");
    assert_eq!(
        body["input"][1]["content"],
        "It uses the sandbox model proxy."
    );
    server.abort();
}

#[tokio::test]
async fn mismatched_request_model_defers_without_contacting_proxy() {
    let calls = Arc::new(AtomicUsize::new(0));
    async fn counted_success(State(calls): State<Arc<AtomicUsize>>) -> Json<serde_json::Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({ "output_text": "must not be used" }))
    }
    let (origin, server) = spawn(
        Router::new()
            .route("/v1/responses", post(counted_success))
            .with_state(Arc::clone(&calls)),
    )
    .await;
    let generator = HttpAgentTitleGenerator::fixture(
        &origin,
        DEPLOYMENT_MODEL,
        Duration::from_secs(2),
        MAX_RESPONSE_BYTES,
    );

    let (outcome, stored) = generate_title(
        &generator,
        "different-agent-model",
        BEARER,
        "model-bound fallback",
        "answer",
    )
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(outcome, Err(AgentTitleServiceError::GenerationUnavailable));
    assert!(stored.is_none());
    server.abort();
}

#[tokio::test]
async fn model_failures_defer_after_one_call_without_committing_the_prompt() {
    let calls = Arc::new(AtomicUsize::new(0));
    async fn unavailable(State(calls): State<Arc<AtomicUsize>>) -> Response {
        calls.fetch_add(1, Ordering::SeqCst);
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "output_text": "must not pass" })),
        )
            .into_response()
    }
    let (origin, server) = spawn(
        Router::new()
            .route("/model-proxy/v1/responses", post(unavailable))
            .with_state(Arc::clone(&calls)),
    )
    .await;
    let generator = HttpAgentTitleGenerator::fixture(
        &format!("{origin}/model-proxy"),
        DEPLOYMENT_MODEL,
        Duration::from_secs(2),
        MAX_RESPONSE_BYTES,
    );
    let owner = format!(
        "{}private owner tail",
        "f".repeat(AGENT_TITLE_FALLBACK_SCALARS)
    );

    let (outcome, stored) =
        generate_title(&generator, "deployment-model", BEARER, &owner, "answer").await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(outcome, Err(AgentTitleServiceError::GenerationUnavailable));
    assert!(stored.is_none());
    server.abort();
}

#[test]
fn standard_response_shapes_are_parsed_and_malformed_shapes_are_refused() {
    assert_eq!(
        response_title(br#"{"output_text":"Direct title"}"#).as_deref(),
        Some("Direct title")
    );
    assert_eq!(
        response_title(
            br#"{"output":[{"type":"reasoning"},{"type":"message","content":[{"type":"refusal","refusal":"no"},{"type":"output_text","text":"Nested title"}]}]}"#
        )
        .as_deref(),
        Some("Nested title")
    );
    for malformed in [
        b"not-json".as_slice(),
        br#"{}"#,
        br#"{"output":[]}"#,
        br#"{"output":[{"type":"message","content":[{"type":"output_text","text":7}]}]}"#,
    ] {
        assert!(response_title(malformed).is_none());
    }
}

#[tokio::test]
async fn malformed_oversized_non_success_timeout_and_redirect_defer_generation() {
    async fn malformed() -> Response {
        ([(header::CONTENT_TYPE, "application/json")], "not-json").into_response()
    }
    async fn oversized() -> Response {
        (
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "output_text": "x".repeat(512) }).to_string(),
        )
            .into_response()
    }
    async fn unavailable() -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "output_text": "must not pass" })),
        )
            .into_response()
    }
    for route in [post(malformed), post(oversized), post(unavailable)] {
        let (origin, server) = spawn(Router::new().route("/v1/responses", route)).await;
        let generator = HttpAgentTitleGenerator::fixture(
            &origin,
            DEPLOYMENT_MODEL,
            Duration::from_secs(2),
            128,
        );
        let (outcome, stored) =
            generate_title(&generator, "deployment-model", BEARER, "fallback", "answer").await;
        assert_eq!(outcome, Err(AgentTitleServiceError::GenerationUnavailable));
        assert!(stored.is_none());
        server.abort();
    }

    async fn delayed() -> Json<serde_json::Value> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Json(serde_json::json!({ "output_text": "too late" }))
    }
    let (origin, server) = spawn(Router::new().route("/v1/responses", post(delayed))).await;
    let generator =
        HttpAgentTitleGenerator::fixture(&origin, DEPLOYMENT_MODEL, Duration::from_millis(20), 128);
    assert_eq!(
        generate_title(&generator, "deployment-model", BEARER, "timeout", "answer")
            .await
            .0,
        Err(AgentTitleServiceError::GenerationUnavailable)
    );
    server.abort();

    async fn redirect() -> impl IntoResponse {
        (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/redirect-target")],
        )
    }
    async fn target(State(calls): State<Arc<AtomicUsize>>) -> StatusCode {
        calls.fetch_add(1, Ordering::SeqCst);
        StatusCode::OK
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let (origin, server) = spawn(
        Router::new()
            .route("/v1/responses", post(redirect))
            .route("/redirect-target", post(target))
            .with_state(Arc::clone(&calls)),
    )
    .await;
    let generator =
        HttpAgentTitleGenerator::fixture(&origin, DEPLOYMENT_MODEL, Duration::from_secs(2), 128);
    assert_eq!(
        generate_title(&generator, "deployment-model", BEARER, "redirect", "answer")
            .await
            .0,
        Err(AgentTitleServiceError::GenerationUnavailable)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn reflected_sandbox_bearer_is_never_accepted_as_title_content() {
    async fn reflected_header() -> Response {
        (
            [
                ("content-type", "application/json"),
                ("x-reflected", BEARER),
            ],
            serde_json::json!({ "output_text": "header reflection" }).to_string(),
        )
            .into_response()
    }
    async fn reflected_body() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "output_text": BEARER }))
    }
    async fn escaped_reflected_body() -> Json<serde_json::Value> {
        Json(serde_json::json!({ "output_text": "sandbox\"title-bearer" }))
    }
    for (route, bearer) in [
        (post(reflected_header), BEARER),
        (post(reflected_body), BEARER),
        (post(escaped_reflected_body), "sandbox\"title-bearer"),
    ] {
        let (origin, server) = spawn(Router::new().route("/v1/responses", route)).await;
        let generator = HttpAgentTitleGenerator::fixture(
            &origin,
            DEPLOYMENT_MODEL,
            Duration::from_secs(2),
            128,
        );
        let (outcome, stored) = generate_title(
            &generator,
            "deployment-model",
            bearer,
            "credential safe fallback",
            "answer",
        )
        .await;
        assert_eq!(outcome, Err(AgentTitleServiceError::GenerationUnavailable));
        assert!(stored.is_none());
        server.abort();
    }
}

#[test]
fn nyxid_model_services_use_the_service_relative_responses_path() {
    for (base_path, expected_path) in [
        (
            "/api/v1/proxy/s/chrono-llm-public/",
            "/api/v1/proxy/s/chrono-llm-public/responses",
        ),
        (
            "/api/v1/proxy/s/chrono-llm",
            "/api/v1/proxy/s/chrono-llm/responses",
        ),
        (
            "/api/v1/proxy/s/chrono-llm-public-preview",
            "/api/v1/proxy/s/chrono-llm-public-preview/v1/responses",
        ),
    ] {
        let endpoint = ProviderEndpoint {
            endpoint: format!("https://identity.example{base_path}"),
        };
        let generator = HttpAgentTitleGenerator::new(&endpoint, DEPLOYMENT_MODEL).unwrap();
        assert_eq!(
            generator.endpoint.as_str(),
            format!("https://identity.example{expected_path}")
        );
    }
}

#[test]
fn proxy_endpoint_is_exact_and_rejects_ambiguous_or_credentialed_urls() {
    let generator = HttpAgentTitleGenerator::new(
        &ProviderEndpoint {
            endpoint: "provider.example/sandboxes/runtime-1/endpoints/9091/".into(),
        },
        DEPLOYMENT_MODEL,
    )
    .expect("a provider-resolved sandbox endpoint is accepted");
    assert_eq!(
        generator.endpoint.as_str(),
        "https://provider.example/sandboxes/runtime-1/endpoints/9091/v1/responses"
    );
    for invalid in [
        "/sandboxes/runtime-1/endpoints/9091",
        "file:///tmp/proxy",
        "https://name:value@provider.example/proxy",
        "https://provider.example/proxy?target=other",
        "https://provider.example/proxy#fragment",
    ] {
        assert!(HttpAgentTitleGenerator::new(
            &ProviderEndpoint {
                endpoint: invalid.into(),
            },
            DEPLOYMENT_MODEL,
        )
        .is_none());
    }
    assert!(HttpAgentTitleGenerator::build(
        "https://provider.example/proxy",
        DEPLOYMENT_MODEL,
        Duration::ZERO,
        MAX_RESPONSE_BYTES,
    )
    .is_none());
    assert!(HttpAgentTitleGenerator::build(
        "https://provider.example/proxy",
        DEPLOYMENT_MODEL,
        Duration::from_secs(1),
        0,
    )
    .is_none());
    assert!(HttpAgentTitleGenerator::build(
        "https://provider.example/proxy",
        "  ",
        Duration::from_secs(1),
        MAX_RESPONSE_BYTES,
    )
    .is_none());
}

#[tokio::test]
async fn provider_routing_authority_is_sensitive_and_never_becomes_a_title() {
    async fn reflected(headers: AxumHeaderMap) -> Json<serde_json::Value> {
        Json(serde_json::json!({"output_text": headers["open-sandbox-api-key"].to_str().unwrap()}))
    }
    let (origin, server) = spawn(Router::new().route("/v1/responses", post(reflected))).await;
    let generator =
        HttpAgentTitleGenerator::fixture(&origin, DEPLOYMENT_MODEL, Duration::from_secs(2), 128)
            .with_proxy_authorization("title-proxy-routing-authority")
            .unwrap();
    assert!(generator
        .proxy_authorization
        .as_ref()
        .unwrap()
        .is_sensitive());
    assert!(!format!("{generator:?}").contains("title-proxy-routing-authority"));
    let (outcome, stored) = generate_title(
        &generator,
        DEPLOYMENT_MODEL,
        BEARER,
        "Safe routing fallback",
        "answer",
    )
    .await;
    assert_eq!(outcome, Err(AgentTitleServiceError::GenerationUnavailable));
    assert!(stored.is_none());
    server.abort();
}

#[test]
fn generation_denials_are_terminal_but_transport_and_rate_limits_are_retryable() {
    for code in [400, 401, 402, 403, 404, 422] {
        assert!(generation_failure(StatusCode::from_u16(code).unwrap(), b"{}").is_permanent());
    }
    for code in [408, 429, 500, 503] {
        assert!(!generation_failure(StatusCode::from_u16(code).unwrap(), b"{}").is_permanent());
    }
    assert!(generation_failure(
        StatusCode::TOO_MANY_REQUESTS,
        br#"{"error":{"code":"insufficient_quota"}}"#
    )
    .is_permanent());
}
