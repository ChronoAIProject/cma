use super::Scenario;
use layer2_core::{bots::BotError, event_workers::*, Authorization, Caller, Capability, SubjectId};
use std::sync::Arc;
struct Unavailable;
#[async_trait::async_trait]
impl EventWorkerProfileStore for Unavailable {
    async fn get(&self, _: &str, _: Option<u32>) -> Result<Option<EventWorkerProfile>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn list(
        &self,
        _: &SubjectId,
        _: Option<&str>,
        _: usize,
    ) -> Result<Vec<EventWorkerProfile>, BotError> {
        Err(BotError::Unavailable)
    }
    async fn save(&self, _: Option<u32>, _: &EventWorkerProfile) -> Result<(), BotError> {
        Err(BotError::Unavailable)
    }
}
pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("event_worker_contract_")?;
    let owner = Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::ManageOwnProfiles]),
    );
    let store: Arc<dyn EventWorkerProfileStore> = if mode == "unavailable" {
        Arc::new(Unavailable)
    } else {
        Arc::new(InMemoryEventWorkerProfileStore::default())
    };
    let service = EventWorkerProfileService::new(
        store,
        Arc::new(super::bot_cases::Profiles),
        "cma_codex".into(),
    );
    let input:EventWorkerProfileInput=serde_json::from_value(serde_json::json!({"name":"Example worker","description":"Complete worker configuration","agent_profile":{"id":"agp_bot","revision":1},"sandbox_profile":{"id":"sbp_bot","revision":1},"workspace_profile":{"id":"wkp_bot","revision":1}})).unwrap();
    if mode != "create" && mode != "invalid" && mode != "unavailable" {
        service
            .create(&owner, "contract-worker", input)
            .await
            .unwrap();
    }
    Some(Scenario {
        router: crate::rest::event_workers::router(service),
        subject: match mode {
            "anonymous" => None,
            "foreign" => Some("foreign"),
            _ => Some("owner"),
        },
        authorization: if mode == "forbidden" {
            Authorization::new([])
        } else {
            owner.authorization
        },
        symbols: Default::default(),
    })
}

#[tokio::test]
async fn list_query_defaults_and_bounds_preserve_typed_errors() {
    let scene = scenario("event_worker_contract_list").await.unwrap();
    let collection = "/api/v1/event-worker-profiles";
    for query in ["", "?limit=1", "?limit=200"] {
        let (status, body) = request(&scene, "GET", &format!("{collection}{query}")).await;
        assert_eq!(status, 200, "{query}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        if query == "?limit=1" {
            let after = body["next_after"].as_str().unwrap();
            let (status, page) = request(
                &scene,
                "GET",
                &format!("{collection}?limit=1&after={after}"),
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(page["items"], serde_json::json!([]));
            assert!(page["next_after"].is_null());
        }
    }
    for limit in ["0", "201", "-1", "invalid"] {
        let (status, body) = request(&scene, "GET", &format!("{collection}?limit={limit}")).await;
        assert_problem(status, &body, 400, "invalid");
    }
}

#[tokio::test]
async fn retire_requires_owner_and_revision_without_mutating_on_rejection() {
    let scene = scenario("event_worker_contract_retire").await.unwrap();
    let (_, list) = request(&scene, "GET", "/api/v1/event-worker-profiles").await;
    let id = list["items"][0]["id"].as_str().unwrap();
    let path = format!("/api/v1/event-worker-profiles/{id}");
    for query in ["", "?expected_revision=invalid"] {
        let (status, body) = request(&scene, "DELETE", &format!("{path}{query}")).await;
        assert_problem(status, &body, 400, "invalid");
    }
    let (status, body) = request(&scene, "DELETE", &format!("{path}?expected_revision=2")).await;
    assert_problem(status, &body, 409, "conflict");
    let foreign = Scenario {
        router: scene.router.clone(),
        subject: Some("foreign"),
        authorization: scene.authorization.clone(),
        symbols: Default::default(),
    };
    let (status, body) = request(&foreign, "DELETE", &format!("{path}?expected_revision=1")).await;
    assert_problem(status, &body, 404, "not_found");
    let (status, body) = request(&scene, "GET", &path).await;
    assert_eq!(status, 200);
    assert_eq!(body["revision"], 1);
    assert!(body["retired_at"].is_null());
    let (status, body) = request(&scene, "DELETE", &format!("{path}?expected_revision=1")).await;
    assert_eq!(status, 200);
    assert_eq!(body["revision"], 2);
    assert!(body["retired_at"].is_string());
}

async fn request(scene: &Scenario, method: &str, path: &str) -> (u16, serde_json::Value) {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    let request = Request::builder()
        .method(method)
        .uri(path)
        .extension(crate::AuthenticatedSubject(SubjectId::new(
            scene.subject.unwrap(),
        )))
        .extension(scene.authorization.clone())
        .body(Body::empty())
        .unwrap();
    let response = scene.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["content-type"],
        if status.is_success() {
            "application/json"
        } else {
            "application/problem+json"
        }
    );
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status.as_u16(), serde_json::from_slice(&bytes).unwrap())
}

fn assert_problem(status: u16, body: &serde_json::Value, expected: u16, reason: &str) {
    let code = format!("event_worker_profile_{reason}");
    assert_eq!(status, expected);
    assert_eq!(body["status"], expected);
    assert_eq!(body["code"], code);
    assert_eq!(body["title"], code);
    assert_eq!(
        body["type"],
        format!("https://spec.chrono-ai.fun/cma/1.0/problems/{code}")
    );
}
