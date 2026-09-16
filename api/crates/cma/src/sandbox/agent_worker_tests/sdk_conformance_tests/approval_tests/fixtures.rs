use super::*;
use axum::body::{to_bytes, Body};
use layer2_core::resource_access::CallerContext;

pub(super) fn context(route: Route, harness: &Harness) -> CallerContext {
    match route {
        Route::Agui => v2_tests::human_context(&harness.workspace.owner),
        Route::Responses | Route::ResponsesV2 => v2_tests::context(&harness.workspace.owner),
    }
}

pub(super) fn authenticated(router: Router, route: Route, harness: &Harness) -> Router {
    router.layer(middleware::from_fn_with_state(
        context(route, harness),
        authenticate_actor,
    ))
}

async fn authenticate_actor(
    axum::extract::State(context): axum::extract::State<CallerContext>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(TEST_BEARER)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            context.credential.subject.clone(),
        ));
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "local-sdk-conformance".into(),
        ));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    Ok(next.run(request).await)
}

pub(super) fn answer_request(
    route: Route,
    agent: &AgentId,
    path: &str,
    intent: &Value,
) -> Request<Body> {
    let (path, body) = match route {
        Route::Agui => (
            "/api/v1/agui/run",
            json!({
                "threadId":agent,
                "resume":[{"interruptId":APPROVAL_ID,"status":"resolved","payload":{"decision":"allow"}}]
            }),
        ),
        Route::Responses | Route::ResponsesV2 => (path, intent.clone()),
    };
    Request::builder()
        .method("POST")
        .uri(path)
        .header("authorization", TEST_BEARER)
        .header("content-type", "application/json")
        .header("idempotency-key", "answer-public-approval")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub(super) async fn answer_admitted(harness: &Harness) -> AdmittedAgentCommand {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let records = harness
                .store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap();
            if let Some(answer) = records.items.into_iter().find(|record| {
                matches!(
                    record.receipt.envelope.command,
                    AgentCommand::AnswerApproval { .. }
                )
            }) {
                break answer;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the public handler admits its approval answer")
}

pub(super) async fn assert_answer_response(
    response: Response,
    route: Route,
    answer: &AdmittedAgentCommand,
) {
    let id = &answer.receipt.envelope.command_id;
    match route {
        Route::Agui => {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["x-cma-command-id"], id.as_str());
            assert!(!response.headers().contains_key("x-cma-response-id"));
        }
        Route::Responses | Route::ResponsesV2 => {
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(status, StatusCode::ACCEPTED, "{value}");
            assert_eq!(value["control"]["receipt"]["command_id"], id.as_str());
            assert_eq!(value["control"]["receipt"]["response_id"], Value::Null);
        }
    }
}

pub(super) async fn snapshot(router: &Router, agent: &AgentId) -> Value {
    serde_json::from_str(&read(router, &format!("/api/v2/agents/{agent}/events/snapshot")).await)
        .unwrap()
}

pub(super) fn assert_approval_binding(snapshot: &Value, kind: &str, source: &AdmittedAgentCommand) {
    let items = snapshot["items"].as_array().unwrap();
    let entries: Vec<_> = items
        .iter()
        .filter(|item| item["kind"]["type"] == kind)
        .collect();
    assert_eq!(entries.len(), 1, "{snapshot}");
    let event = entries[0];
    assert_eq!(event["kind"]["approval_id"], APPROVAL_ID);
    // Rich replay binds the source once at TurnStarted. Approval events belong
    // to that turn by ordered history; they have no independent response binding.
    assert!(event["response_binding"].is_null());
    let turn = source.work.dispatch.as_ref().unwrap().turn_sequence;
    let start = items.iter().find(|item| item["sequence"] == turn).unwrap();
    assert_eq!(start["kind"]["type"], "turn_started");
    assert_eq!(
        start["response_binding"]["command_id"],
        source.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        start["response_binding"]["response_id"],
        source.receipt.response_id.as_ref().unwrap().as_str()
    );
    assert_eq!(start["response_binding"]["turn_sequence"], turn);
    let sequence = event["sequence"].as_u64().unwrap();
    assert!(sequence > turn);
    assert!(!items.iter().any(|item| {
        let at = item["sequence"].as_u64().unwrap();
        at > turn
            && at < sequence
            && matches!(
                item["kind"]["type"].as_str(),
                Some("turn_started" | "turn_completed")
            )
    }));
    assert!(!snapshot.to_string().contains(APPROVAL_LOCATOR));
}

pub(super) async fn wrong_approval_is_refused(router: &Router, path: &str, intent: &Value) {
    let mut wrong = intent.clone();
    wrong["command"]["approval_id"] = json!("another-approval");
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", TEST_BEARER)
                .header("content-type", "application/json")
                .header("idempotency-key", "wrong-public-approval")
                .body(Body::from(wrong.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    assert_eq!(value["error"]["code"], "response_control_conflict");
}
