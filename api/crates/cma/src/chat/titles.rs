//! Strict HTTP adapter for automatic chat-thread naming.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt as _;
use layer2_core::{
    ChatTitleGenerationError, ChatTitleGenerationRequest, ChatTitleGenerator,
    CHAT_TITLE_CALL_TIMEOUT_SECONDS, CHAT_TITLE_SYSTEM_PROMPT,
};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE,
};
use reqwest::{StatusCode, Url};
use serde::Serialize;

const RESPONSES_PATH: &str = "/v1/responses";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_OUTPUT_TOKENS: u32 = 32;

#[derive(Clone)]
pub(crate) struct HttpChatTitleGenerator {
    endpoint: Url,
    fallback_model: Option<String>,
    client: reqwest::Client,
    maximum_response_body: usize,
}

impl fmt::Debug for HttpChatTitleGenerator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpChatTitleGenerator")
            .field("endpoint", &self.endpoint)
            .field("fallback_model", &self.fallback_model)
            .field("maximum_response_body", &self.maximum_response_body)
            .finish_non_exhaustive()
    }
}

impl HttpChatTitleGenerator {
    pub(crate) fn new(base_url: &str, fallback_model: &str) -> Option<Self> {
        Self::build(
            base_url,
            fallback_model,
            Duration::from_secs(CHAT_TITLE_CALL_TIMEOUT_SECONDS),
            MAX_RESPONSE_BYTES,
        )
    }

    fn build(
        base_url: &str,
        fallback_model: &str,
        request_timeout: Duration,
        maximum_response_body: usize,
    ) -> Option<Self> {
        let endpoint = title_endpoint(base_url)?;
        if request_timeout.is_zero() || maximum_response_body == 0 {
            return None;
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT.min(request_timeout))
            .timeout(request_timeout)
            .build()
            .ok()?;
        Some(Self {
            endpoint,
            fallback_model: nonempty(fallback_model).map(str::to_owned),
            client,
            maximum_response_body,
        })
    }

    #[cfg(test)]
    fn fixture(
        base_url: &str,
        fallback_model: &str,
        timeout: Duration,
        maximum_response_body: usize,
    ) -> Self {
        Self::build(base_url, fallback_model, timeout, maximum_response_body)
            .expect("the title-generator fixture configuration is valid")
    }

    async fn send(
        &self,
        request: &ChatTitleGenerationRequest,
    ) -> Result<String, ChatTitleGenerationError> {
        let model = request
            .model()
            .and_then(nonempty)
            .or(self.fallback_model.as_deref())
            .ok_or_else(ChatTitleGenerationError::unavailable)?;
        let body = serde_json::to_vec(&ResponsesRequest {
            model,
            instructions: CHAT_TITLE_SYSTEM_PROMPT,
            input: [
                ResponsesInput {
                    role: "user",
                    content: request.owner_message(),
                },
                ResponsesInput {
                    role: "assistant",
                    content: request.assistant_message(),
                },
            ],
            stream: false,
            store: false,
            max_output_tokens: MAX_OUTPUT_TOKENS,
        })
        .map_err(|_| ChatTitleGenerationError::unavailable())?;
        if body.len() > MAX_REQUEST_BYTES {
            return Err(ChatTitleGenerationError::unavailable());
        }

        let authorization = HeaderValue::from_str(&format!("Bearer {}", request.bearer()))
            .map_err(|_| ChatTitleGenerationError::unavailable())?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));

        let response = self
            .client
            .post(self.endpoint.clone())
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|_| ChatTitleGenerationError::unavailable())?;
        if response.status() != StatusCode::OK
            || response.headers().get_all(CONTENT_TYPE).iter().count() != 1
            || response
                .headers()
                .get(CONTENT_TYPE)
                .is_none_or(|value| value.as_bytes() != b"application/json")
            || response.headers().contains_key(CONTENT_ENCODING)
            || headers_contain(response.headers(), request.bearer().as_bytes())
        {
            return Err(ChatTitleGenerationError::unavailable());
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ChatTitleGenerationError::unavailable())?;
            let size = body
                .len()
                .checked_add(chunk.len())
                .ok_or_else(ChatTitleGenerationError::unavailable)?;
            if size > self.maximum_response_body {
                return Err(ChatTitleGenerationError::unavailable());
            }
            body.extend_from_slice(&chunk);
        }
        if contains_bytes(&body, request.bearer().as_bytes()) {
            return Err(ChatTitleGenerationError::unavailable());
        }
        response_title(&body).ok_or_else(ChatTitleGenerationError::unavailable)
    }
}

#[async_trait]
impl ChatTitleGenerator for HttpChatTitleGenerator {
    async fn generate(
        &self,
        request: ChatTitleGenerationRequest,
    ) -> Result<String, ChatTitleGenerationError> {
        self.send(&request).await
    }
}

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    instructions: &'static str,
    input: [ResponsesInput<'a>; 2],
    stream: bool,
    store: bool,
    max_output_tokens: u32,
}

#[derive(Serialize)]
struct ResponsesInput<'a> {
    role: &'static str,
    content: &'a str,
}

fn title_endpoint(base_url: &str) -> Option<Url> {
    let base = Url::parse(base_url.trim_end_matches('/')).ok()?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return None;
    }
    Url::parse(&format!(
        "{}{RESPONSES_PATH}",
        base.as_str().trim_end_matches('/')
    ))
    .ok()
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn response_title(body: &[u8]) -> Option<String> {
    let response: serde_json::Value = serde_json::from_slice(body).ok()?;
    if let Some(title) = response.get("output_text").and_then(|value| value.as_str()) {
        return Some(title.to_owned());
    }
    response
        .get("output")?
        .as_array()?
        .iter()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("message"))
        .filter_map(|item| item.get("content").and_then(|value| value.as_array()))
        .flatten()
        .find(|content| {
            content.get("type").and_then(|value| value.as_str()) == Some("output_text")
        })?
        .get("text")?
        .as_str()
        .map(str::to_owned)
}

fn headers_contain(headers: &HeaderMap, protected: &[u8]) -> bool {
    headers.iter().any(|(name, value)| {
        contains_bytes(name.as_str().as_bytes(), protected)
            || contains_bytes(value.as_bytes(), protected)
    })
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{header, HeaderMap as AxumHeaderMap};
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Capture {
        authorization: Mutex<Option<String>>,
        body: Mutex<Option<serde_json::Value>>,
    }

    fn request(model: Option<&str>) -> ChatTitleGenerationRequest {
        ChatTitleGenerationRequest::new(
            model.map(str::to_owned),
            "first private owner input",
            "first private assistant output",
            "bearer-fixture-value".into(),
        )
    }

    async fn spawn(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (origin, task)
    }

    async fn capture_success(
        State(capture): State<Arc<Capture>>,
        headers: AxumHeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        *capture.authorization.lock().unwrap() = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        *capture.body.lock().unwrap() = Some(body);
        Json(serde_json::json!({ "output_text": "Pinned Model Title" }))
    }

    #[tokio::test]
    async fn proxy_path_bearer_fixed_prompt_roles_and_pinned_model_are_exact() {
        let capture = Arc::new(Capture::default());
        let (origin, server) = spawn(
            Router::new()
                .route(
                    "/api/v1/proxy/s/chrono-llm/v1/responses",
                    post(capture_success),
                )
                .with_state(Arc::clone(&capture)),
        )
        .await;
        let generator = HttpChatTitleGenerator::fixture(
            &format!("{origin}/api/v1/proxy/s/chrono-llm/"),
            "deployment-model",
            Duration::from_secs(2),
            MAX_RESPONSE_BYTES,
        );

        let title = generator
            .generate(request(Some("pinned-chat-model")))
            .await
            .unwrap();

        assert_eq!(title, "Pinned Model Title");
        assert_eq!(
            capture.authorization.lock().unwrap().as_deref(),
            Some("Bearer bearer-fixture-value")
        );
        let body = capture.body.lock().unwrap().clone().unwrap();
        assert_eq!(body["model"], "pinned-chat-model");
        assert_eq!(body["instructions"], CHAT_TITLE_SYSTEM_PROMPT);
        assert_eq!(body["stream"], false);
        assert_eq!(body["store"], false);
        assert_eq!(body["max_output_tokens"], MAX_OUTPUT_TOKENS);
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"], "first private owner input");
        assert_eq!(body["input"][1]["role"], "assistant");
        assert_eq!(
            body["input"][1]["content"],
            "first private assistant output"
        );
        server.abort();
    }

    #[tokio::test]
    async fn deployment_model_is_used_only_when_the_pinned_model_is_absent() {
        let capture = Arc::new(Capture::default());
        let (origin, server) = spawn(
            Router::new()
                .route("/v1/responses", post(capture_success))
                .with_state(Arc::clone(&capture)),
        )
        .await;
        let generator = HttpChatTitleGenerator::fixture(
            &origin,
            "deployment-model",
            Duration::from_secs(2),
            MAX_RESPONSE_BYTES,
        );

        generator.generate(request(None)).await.unwrap();

        assert_eq!(
            capture.body.lock().unwrap().as_ref().unwrap()["model"],
            "deployment-model"
        );
        server.abort();
    }

    #[test]
    fn standard_output_text_and_message_content_shapes_are_parsed() {
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
    async fn malformed_oversized_non_success_timeout_and_redirect_responses_fail_closed() {
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
        async fn delayed() -> Json<serde_json::Value> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Json(serde_json::json!({ "output_text": "too late" }))
        }
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

        for route in [post(malformed), post(oversized), post(unavailable)] {
            let (origin, server) = spawn(Router::new().route("/v1/responses", route)).await;
            let generator = HttpChatTitleGenerator::fixture(
                &origin,
                "deployment-model",
                Duration::from_secs(2),
                128,
            );
            assert_eq!(
                generator.generate(request(None)).await.unwrap_err(),
                ChatTitleGenerationError::unavailable()
            );
            server.abort();
        }

        let (origin, server) = spawn(Router::new().route("/v1/responses", post(delayed))).await;
        let generator = HttpChatTitleGenerator::fixture(
            &origin,
            "deployment-model",
            Duration::from_millis(20),
            MAX_RESPONSE_BYTES,
        );
        assert!(generator.generate(request(None)).await.is_err());
        server.abort();

        let calls = Arc::new(AtomicUsize::new(0));
        let (origin, server) = spawn(
            Router::new()
                .route("/v1/responses", post(redirect))
                .route("/redirect-target", post(target))
                .with_state(Arc::clone(&calls)),
        )
        .await;
        let generator = HttpChatTitleGenerator::fixture(
            &origin,
            "deployment-model",
            Duration::from_secs(2),
            MAX_RESPONSE_BYTES,
        );
        assert!(generator.generate(request(None)).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn reflected_bearer_and_sensitive_debug_content_are_rejected() {
        async fn reflected() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "output_text": "bearer-fixture-value"
            }))
        }
        let (origin, server) = spawn(Router::new().route("/v1/responses", post(reflected))).await;
        let generator = HttpChatTitleGenerator::fixture(
            &origin,
            "deployment-model",
            Duration::from_secs(2),
            MAX_RESPONSE_BYTES,
        );
        let error = generator.generate(request(None)).await.unwrap_err();
        let debug = format!("{generator:?} {error:?}");
        for private in [
            "bearer-fixture-value",
            "first private owner input",
            "first private assistant output",
        ] {
            assert!(!debug.contains(private));
        }
        server.abort();
    }

    #[test]
    fn endpoint_configuration_is_absolute_bounded_to_the_prefix_and_secret_free() {
        assert_eq!(
            title_endpoint("https://platform.example/api/v1/proxy/s/chrono-llm/")
                .unwrap()
                .as_str(),
            "https://platform.example/api/v1/proxy/s/chrono-llm/v1/responses"
        );
        for invalid in [
            "",
            "/api/v1/proxy/s/chrono-llm",
            "file:///tmp/proxy",
            "https://name:value@platform.example/proxy",
            "https://platform.example/proxy?target=other",
            "https://platform.example/proxy#fragment",
        ] {
            assert!(title_endpoint(invalid).is_none(), "accepted {invalid}");
        }
    }
}
