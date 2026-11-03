#[path = "public_contract/agent_browser.rs"]
mod agent_browser;
#[path = "public_contract/agent_events_v2.rs"]
mod agent_events_v2;
#[path = "public_contract/bindings.rs"]
mod bindings;
#[path = "public_contract/bots.rs"]
mod bots;
#[path = "public_contract/context.rs"]
mod context;
#[path = "public_contract/deliverables.rs"]
mod deliverables;
#[path = "public_contract/documents.rs"]
mod documents;
#[path = "public_contract/event_workers.rs"]
mod event_workers;
#[path = "public_contract/experience.rs"]
mod experience;
#[path = "public_contract/operator.rs"]
mod operator;
#[path = "public_contract/resources.rs"]
mod resources;
#[path = "public_contract/responses_v2.rs"]
mod responses_v2;
#[path = "public_contract/triggers.rs"]
mod triggers;
#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{Error, Result};
use context::{Case, Context};

async fn dispatch(c: &Context<'_>, corpus: &[Case]) -> Result<()> {
    let op = c.case.operation.as_str();
    if op.ends_with("/browser") && op.contains("/api/v1/agents/") {
        agent_browser::dispatch(c).await
    } else if op.contains("/deliverable-references") || op.contains("/deliverables") {
        deliverables::dispatch(c, corpus).await
    } else if op.contains("/api/v1/documents") {
        documents::dispatch(c).await
    } else if op.contains("/event-worker-profiles") {
        event_workers::dispatch(c).await
    } else if op.contains("/api/v1/bots")
        || op.contains("/api/v1/bot-setups")
        || op.contains("/api/v1/agent-delegations")
        || op.contains("/gateway/interaction-records")
    {
        bots::dispatch(c).await
    } else if op.contains("/cmaeg/bindings") {
        bindings::dispatch(c).await
    } else if op == "GET /api/v2/ui/transport"
        || op == "GET /api/v2/agents/{agent_id}/events"
        || op == "GET /api/v2/agents/{agent_id}/events/snapshot"
        || op == "GET /api/v2/agents/{agent_id}/events/head"
    {
        agent_events_v2::dispatch(c).await
    } else if op.contains("/api/v2") {
        responses_v2::dispatch(c).await
    } else if op.contains("/admin/")
        || op.contains("/profile-authoring/")
        || [
            "GET /api/v1/conformance",
            "GET /api/v1/defaults",
            "GET /openapi.json",
            "POST /api/v1/mcp",
            "POST /api/v1/a2a",
            "GET /.well-known/agent-card.json",
            "GET /api/v1/agents/{id}/terminal",
        ]
        .contains(&op)
    {
        operator::dispatch(c).await
    } else if op.contains("/triggers")
        || op.contains("/launches")
        || op.contains("/service-connections")
    {
        triggers::dispatch(c).await
    } else if op.contains("/agui/")
        || op.contains("/responses")
        || op.contains("/conversations")
        || op.starts_with("GET /auth/")
    {
        experience::dispatch(c).await
    } else {
        resources::dispatch(c).await
    }
}

fn check_error(case: &Case, error: Error) {
    let v2 = case.response.body.get("error").is_some();
    let expected = if v2 {
        &case.response.body["error"]
    } else {
        &case.response.body
    };
    let Error::Api {
        status,
        code,
        extensions,
        ..
    } = error
    else {
        panic!("{} expected API refusal, got {error:?}", case.id)
    };
    assert_eq!(status, case.response.status, "{} status", case.id);
    assert_eq!(
        code.as_deref(),
        expected["code"].as_str(),
        "{} code",
        case.id
    );
    let mut expected = expected.as_object().unwrap().clone();
    let standard = if v2 {
        vec!["code", "message"]
    } else {
        vec!["type", "title", "detail", "status", "code", "instance"]
    };
    for key in standard {
        expected.remove(key);
    }
    assert_eq!(
        serde_json::to_value(extensions).unwrap(),
        serde_json::Value::Object(expected),
        "{} extensions",
        case.id
    );
}

#[tokio::test]
async fn shared_backend_corpus_replays_through_typed_sdk_methods() {
    let corpus = context::load();
    let mut replayed = 0;
    let mut preflight = 0;
    let mut wire_shape = 0;
    let mut browser_owned = 0;
    if std::env::var_os("CMA_SDK_CONTRACT_FILE").is_none() {
        for (id, _) in classification::LOCAL_CASES {
            assert!(
                corpus.iter().any(|case| case.id == *id),
                "stale local classification {id}"
            );
        }
        for id in browser::BROWSER_CASES {
            assert!(
                corpus.iter().any(|case| case.id == *id),
                "stale browser classification {id}"
            );
        }
    }
    for case in &corpus {
        let server = wire::WireServer::new().await;
        let c = Context::new(&server.anonymous, &case);
        if browser::owns(&case) {
            browser::verify(&c);
            assert_eq!(
                server.request_count(),
                0,
                "{} browser-owned case emitted HTTP",
                case.id
            );
            browser_owned += 1;
            continue;
        }
        if let Some(class) = classification::classify(&case) {
            if matches!(
                class,
                classification::Classification::Local | classification::Classification::TypedShape
            ) {
                let Err(Error::Validation { message }) = dispatch(&c, &corpus).await else {
                    panic!("{} must reject before HTTP", case.id)
                };
                let fixture_shape = message.starts_with("fixture shape: ");
                assert_eq!(
                    fixture_shape,
                    class == classification::Classification::TypedShape,
                    "{} construction/preflight classification: {message}",
                    case.id
                );
                if fixture_shape {
                    wire_shape += 1;
                } else {
                    preflight += 1;
                }
            } else {
                classification::assert_unrepresentable(&case, class);
                wire_shape += 1;
            }
            assert_eq!(
                server.request_count(),
                0,
                "{} local case emitted HTTP",
                case.id
            );
            check_error(
                &case,
                Error::from_http_problem(
                    case.response.status,
                    case.response.headers.get("x-request-id").cloned(),
                    case.response.headers.get("retry-after").map(String::as_str),
                    &serde_json::to_vec(&case.response.body).unwrap(),
                ),
            );
            continue;
        }
        let exchange = deliverables::exchange(case, &corpus);
        for step in &exchange {
            server.expect(context::wire(step));
        }
        let result = dispatch(&c, &corpus).await;
        if case.response.status >= 400 && case.response.body_kind == "empty" {
            let error = result.expect_err(&case.id);
            assert!(
                matches!(error, Error::Protocol { .. }),
                "{}: {error:?}",
                case.id
            );
            server.assert_drained();
            assert_eq!(
                server.request_count(),
                1,
                "{} must use exactly one typed HTTP request",
                case.id
            );
        } else if case.response.status >= 400
            && !(case.operation == "GET /auth/session" && case.response.status == 401)
        {
            check_error(&case, result.expect_err(&case.id));
        } else {
            result.unwrap_or_else(|error| panic!("{}: {error:?}", case.id));
        }
        server.assert_drained();
        assert_eq!(
            server.request_count(),
            exchange.len(),
            "{} must use the complete typed HTTP exchange",
            case.id
        );
        let _ = (&server.client, &server.base_url);
        replayed += 1;
    }
    eprintln!(
        "Shared Rust corpus: {replayed} typed HTTP, {preflight} real typed preflight, {wire_shape} unrepresentable wire shapes, {browser_owned} browser-owned response/navigation contracts"
    );
}
#[path = "public_contract/browser.rs"]
mod browser;
#[path = "public_contract/classification.rs"]
mod classification;
