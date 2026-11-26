use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::hmac;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::net::TcpListener;
use url::Url;

use crate::{
    config::Config,
    ledger::{Claim, Ledger},
    model::{now, Check, Mode, Reason},
    protocol::{Outcome, Protocol},
    runner::first,
};

use super::{channel, config, gateway_config_value, job};

enum Fault {
    None,
    GatewayPrefix,
    Html,
    Interrupted,
    MissingReceipt,
    NoTerminal,
    Credentials,
    WrongOutputId,
}

#[tokio::test]
async fn gateway_checks_use_the_public_api_prefix_for_ingress_and_evidence() {
    let dir = TempDir::new().unwrap();
    let (server, task) = serve(Fault::GatewayPrefix).await;
    let path = dir.path().join("config.json");
    let mut value = gateway_config_value(&server.base, dir.path());
    std::fs::write(&path, value.to_string()).unwrap();
    let config = Arc::new(Config::load(&path).unwrap());
    let protocol = Protocol::new(config.clone()).unwrap();
    for (check, mode) in [
        (Check::GatewayEvents, Mode::Read),
        (Check::Gateway, Mode::Deterministic),
    ] {
        let intent = protocol.intent(job(&config, check, mode), now()).unwrap();
        let result = first(&protocol, &intent).await;
        assert_eq!(result.reason, Reason::Passed, "{}", intent.job.key());
        if mode == Mode::Deterministic {
            assert!(result.evidence.receipt_at.is_some());
            assert!(result.evidence.terminal_at.is_some());
            assert!(result.evidence.delivery_at.is_some());
        }
    }
    assert_eq!(server.admissions.lock().unwrap().len(), 1);

    value["deterministic"]["gateway"]["ingress"] =
        json!(server.base.join("gateway-other/custom/det").unwrap());
    std::fs::write(&path, value.to_string()).unwrap();
    assert!(matches!(
        Config::load(&path),
        Err("invalid or shared synthetic targets")
    ));
    task.abort();
}

struct Fixture {
    base: Url,
    fault: Fault,
    admissions: Mutex<HashMap<String, (String, String)>>,
}

async fn serve(fault: Fault) -> (Arc<Fixture>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let fixture = Arc::new(Fixture {
        base,
        fault,
        admissions: Mutex::new(HashMap::new()),
    });
    let app = Router::new().fallback(handler).with_state(fixture.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (fixture, server)
}

#[tokio::test]
async fn public_contracts_deliver_and_retain_the_deterministic_slot_marker() {
    let dir = TempDir::new().unwrap();
    let (server, task) = serve(Fault::None).await;
    let config = config(&server.base, dir.path());
    let protocol = Protocol::new(config.clone()).unwrap();
    for (job, enabled) in config.jobs() {
        if !enabled {
            continue;
        }
        let intent = protocol.intent(job, now()).unwrap();
        let result = first(&protocol, &intent).await;
        assert_eq!(result.reason, Reason::Passed, "{}", job.key());
        if job.mode != Mode::Read {
            assert!(
                result.evidence.receipt_at.is_some()
                    && result.evidence.terminal_at.is_some()
                    && result.evidence.delivery_at.is_some()
            );
        }
    }
    assert_eq!(server.admissions.lock().unwrap().len(), 3);
    task.abort();
}

#[tokio::test]
async fn failures_are_classified_and_restart_cannot_resend() {
    let cases = [
        (Fault::Html, Check::Web, Mode::Read, Reason::Contract),
        (
            Fault::Credentials,
            Check::Agent,
            Mode::Read,
            Reason::CredentialExpired,
        ),
        (
            Fault::MissingReceipt,
            Check::Agui,
            Mode::Deterministic,
            Reason::ReceiptMissing,
        ),
        (
            Fault::NoTerminal,
            Check::Agui,
            Mode::Deterministic,
            Reason::DeliveryLost,
        ),
        (
            Fault::Interrupted,
            Check::Agui,
            Mode::Deterministic,
            Reason::DeliveryLost,
        ),
    ];
    for (fault, check, mode, failure) in cases {
        let dir = TempDir::new().unwrap();
        let (server, task) = serve(fault).await;
        let config = config(&server.base, dir.path());
        let protocol = Protocol::new(config.clone()).unwrap();
        let ledger = Ledger::new(&config);
        ledger.init().unwrap();
        ledger.configure(&config.jobs()).unwrap();
        let job = job(&config, check, mode);
        let slot = now();
        let intent = protocol.intent(job, slot).unwrap();
        assert!(matches!(
            ledger.claim(&intent, slot, None).unwrap(),
            Claim::Send
        ));
        let observed = first(&protocol, &intent).await;
        assert_eq!(observed.reason, failure);
        if matches!(server.fault, Fault::Interrupted) {
            // The request reached the server, then the process stops before persisting its result.
            let restarted = Ledger::new(&config);
            assert!(matches!(
                restarted.claim(&intent, slot, None).unwrap(),
                Claim::Seen
            ));
            let pending = restarted.recovery(slot + 35).unwrap();
            assert_eq!(pending.len(), 1);
            let mut outcome = Outcome::default();
            protocol
                .verify(&pending[0].intent, &mut outcome)
                .await
                .unwrap();
            assert!(outcome.settled);
            assert_eq!(server.admissions.lock().unwrap().len(), 1);
        } else {
            ledger.first(&intent, &observed).unwrap();
        }
        task.abort();
    }
}

async fn handler(State(state): State<Arc<Fixture>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    let path = if matches!(state.fault, Fault::GatewayPrefix) {
        let Some(path) = path
            .strip_prefix("/gateway")
            .filter(|path| path.starts_with('/'))
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        path
    } else {
        path
    };
    let bytes = to_bytes(body, 16 * 1024).await.unwrap();
    match path {
        "/" => html(if matches!(state.fault, Fault::Html) { "<html><h1>Error</h1></html>" } else { "<html><head><link rel=stylesheet href=/app.css><script type=module src=/app.js></script></head><body><div id=root></div></body></html>" }),
        "/app.css" => ([("content-type", "text/css")], "body { color: black; }").into_response(),
        "/app.js" => ([("content-type", "text/javascript")], "let ready = true;").into_response(),
        "/.well-known/openid-configuration" => Json(json!({"issuer":state.base,"authorization_endpoint":state.base.join("/auth").unwrap(),"token_endpoint":state.base.join("/token").unwrap(),"jwks_uri":state.base.join("/jwks").unwrap()})).into_response(),
        "/auth/session" => Json(json!({"authenticated":true,"subject":"subject_test"})).into_response(),
        "/api/v1/agents/agt_read" if matches!(state.fault, Fault::Credentials) => (StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer error_description=expired")]).into_response(),
        "/api/v1/agents/agt_read" => Json(json!({"id":"agt_read","sandbox":{"id":"sbx_read"},"status":"ready"})).into_response(),
        "/api/v1/agui/events" => {
            let query = parts.uri.query().unwrap();
            assert!(query.contains("follow=false") && query.contains("starting_after=2"));
            if query.contains("agt_read") { agui("agt_read", "reference-answer", false) }
            else { agui("agt_agui_det", &answer(&state, "agt_agui_det"), matches!(state.fault, Fault::NoTerminal)) }
        }
        path if path == format!("/v1/channels/{}/activity/stream", channel("read")) => sse(format!("id: page\nevent: activity\ndata: {}\n\n", json!({
            "cursor":"page","has_more":false,"limit":50,"retention_days":7,"items":[{"event_id":"event_read","received_at":AT,"consumers":[{"consumer_id":"consumer_read","state":"completed"}]}]}))),
        "/api/v1/agui/run" => {
            assert_eq!(parts.method, Method::POST);
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let marker = marker(body["messages"][0]["content"].as_str().unwrap());
            admit(&state, &parts.headers, &body, "agt_agui_det", marker);
            if matches!(state.fault, Fault::Interrupted) {
                with_agui_headers(sse("data: {\"type\":\"RUN_STARTED\",\"threadId\":\"agt_agui_det\",\"runId\":\"3\"}\n\ndata: {".into()))
            } else {
                agui("agt_agui_det", marker, matches!(state.fault, Fault::NoTerminal))
            }
        }
        "/api/v1/agui/run/receipt" => {
            exact_lookup(&state, &parts.headers, &bytes);
            if matches!(state.fault, Fault::MissingReceipt) { Json(Value::Null).into_response() }
            else { Json(json!({"threadId":"agt_agui_det","runId":"3","status":"completed"})).into_response() }
        }

        "/api/v2/agents/agt_responses_det/responses" | "/api/v2/agents/agt_responses_paid/responses" => {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let agent = body["conversation"].as_str().unwrap();
            let prompt = body["input"].as_str().unwrap();
            let test_answer = if agent.ends_with("_det") { marker(prompt) } else { "Funded short answer" };
            admit(&state, &parts.headers, &body, agent, test_answer);
            responses(agent, test_answer)
        }
        "/api/v2/agents/agt_responses_det/responses/receipt" | "/api/v2/agents/agt_responses_paid/responses/receipt" => {
            exact_lookup(&state, &parts.headers, &bytes);
            let agent = path.split('/').nth(4).unwrap();
            Json(receipt(agent)).into_response()
        }
        "/api/v2/commands/cmd_agui" => Json(receipt("agt_agui_det")).into_response(),
        "/api/v2/commands/cmd_responses_det" | "/api/v2/commands/cmd_responses_paid" => Json(receipt(&response_agent(path))).into_response(),
        "/api/v2/responses/resp_responses_det" | "/api/v2/responses/resp_responses_paid" => {
            let agent = response_agent(path);
            Json(response(&agent, "completed", &[item(&answer(&state, &agent))])).into_response()
        }
        "/api/v2/responses/resp_responses_det/output" | "/api/v2/responses/resp_responses_paid/output" => {
            let agent = response_agent(path);
            let admitted = response_id(&agent);
            let final_page = parts.uri.query().is_some();
            let wrong = matches!(state.fault, Fault::WrongOutputId) && final_page;
            let text = answer(&state, &agent);
            let split = text.len() / 2;
            let output = if final_page { &text[split..] } else { &text[..split] };
            Json(json!({"object":"cma.response.output_page","response_id":if wrong { "resp_other" } else { &admitted },
                "data":[item(output)],"has_more":!final_page,"next_after":(!final_page).then_some("indexed:2")})).into_response()
        }
        "/custom/det" => gateway(&state, &parts.headers, &bytes),
        path if path == format!("/v1/channels/{}/activity/history", channel("det")) => {
            let state_map = state.admissions.lock().unwrap();
            let (_, (body, _)) = state_map.iter().find(|(key,_)| !key.contains('/')).unwrap();
            let cloud: Value = serde_json::from_str(body).unwrap();
            Json(json!({"has_more":false,"items":[{"activity":{"event_id":cloud["id"],"source":cmaeg_core::activity::SourceKind::Custom,"actor":{"id":cloud["source"]},"event_type":"journey.test","received_at":AT,"consumers":[{"consumer_id":"consumer_det","state":"completed","conversation_id":"conv_gateway","response_id":"resp_gateway","accepted_at":AT,"completed_at":AT,"deliveries":[{"step":"answer","destination":"origin","outcome":"buffered"}]}]}}]})).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

const AT: &str = "2026-01-02T00:00:00Z";

fn html(body: &'static str) -> Response {
    ([("content-type", "text/html")], body).into_response()
}
fn sse(body: String) -> Response {
    ([("content-type", "text/event-stream")], Body::from(body)).into_response()
}
fn marker(input: &str) -> &str {
    input.strip_prefix("Return this exact text: ").unwrap()
}

fn admit(state: &Fixture, headers: &HeaderMap, body: &Value, agent: &str, marker: &str) {
    let key = headers["idempotency-key"].to_str().unwrap();
    let mut values = state.admissions.lock().unwrap();
    // A retry using either the original or a replacement key would duplicate this fixture effect.
    assert!(values
        .insert(
            format!("{agent}/{key}"),
            (body.to_string(), marker.to_owned())
        )
        .is_none());
}

fn answer(state: &Fixture, agent: &str) -> String {
    state
        .admissions
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| k.starts_with(&format!("{agent}/")))
        .unwrap()
        .1
         .1
        .clone()
}

fn exact_lookup(state: &Fixture, headers: &HeaderMap, bytes: &[u8]) {
    let key = headers["idempotency-key"].to_str().unwrap();
    let values = state.admissions.lock().unwrap();
    let (_, (body, _)) = values
        .iter()
        .find(|(k, _)| k.ends_with(&format!("/{key}")))
        .unwrap();
    assert_eq!(body.as_bytes(), bytes);
}

fn agui(agent: &str, answer: &str, omit_terminal: bool) -> Response {
    let mut events = vec![
        json!({"type":"RUN_STARTED"}),
        json!({"type":"TEXT_MESSAGE_START","messageId":"msg_4","role":"assistant"}),
        json!({"type":"TEXT_MESSAGE_CONTENT","messageId":"msg_4","delta":answer}),
        json!({"type":"TEXT_MESSAGE_END","messageId":"msg_4"}),
    ];
    if !omit_terminal {
        events.push(json!({"type":"RUN_FINISHED","outcome":{"type":"complete"}}));
    }
    let wire = events
        .into_iter()
        .map(|mut event| {
            event["threadId"] = json!(agent);
            event["runId"] = json!("3");
            format!("data: {event}\n\n")
        })
        .collect();
    with_agui_headers(sse(wire))
}

fn with_agui_headers(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("x-cma-command-id", "cmd_agui".parse().unwrap());
    response
        .headers_mut()
        .insert("x-cma-response-id", "resp_agui".parse().unwrap());
    response
}

fn receipt(agent: &str) -> Value {
    let alias = if agent == "agt_agui_det" {
        "agui"
    } else {
        agent.trim_start_matches("agt_")
    };
    json!({"object":"cma.agent.command","turn_sequence":3,"receipt":{"agent_id":agent,"command_id":format!("cmd_{alias}"),"response_id":format!("resp_{alias}"),"agent_order":1,"event_cursor":0,"admitted_at":AT}})
}

fn part(text: &str) -> Value {
    json!({"type":"output_text","text":text})
}
fn item(text: &str) -> Value {
    json!({"type":"message","id":"msg_resp","status":"completed","role":"assistant","content":[part(text)]})
}
fn response(agent: &str, status: &str, output: &[Value]) -> Value {
    json!({"object":"response","id":response_id(agent),"status":status,"conversation":{"id":agent},"output":output,"completed_at":42})
}

fn responses(agent: &str, answer: &str) -> Response {
    let frames = [
        (
            "response.created",
            json!({"response":response(agent, "queued", &[])}),
        ),
        (
            "response.in_progress",
            json!({"response":response(agent, "in_progress", &[])}),
        ),
        (
            "response.output_item.added",
            json!({"output_index":0,"item":{"type":"message","id":"msg_resp"}}),
        ),
        (
            "response.content_part.added",
            json!({"output_index":0,"item_id":"msg_resp","content_index":0,"part":part("")}),
        ),
        (
            "response.output_text.delta",
            json!({"output_index":0,"item_id":"msg_resp","content_index":0,"delta":answer}),
        ),
        (
            "response.output_text.done",
            json!({"output_index":0,"item_id":"msg_resp","content_index":0,"text":answer}),
        ),
        (
            "response.content_part.done",
            json!({"output_index":0,"item_id":"msg_resp","content_index":0,"part":part(answer)}),
        ),
        (
            "response.output_item.done",
            json!({"output_index":0,"item":item(answer)}),
        ),
        (
            "response.completed",
            json!({"response":response(agent, "completed", &[item(answer)])}),
        ),
    ];
    let wire = frames
        .into_iter()
        .enumerate()
        .map(|(sequence, (kind, mut value))| {
            value["type"] = json!(kind);
            value["sequence_number"] = json!(sequence);
            format!("id: {sequence}\nevent: {kind}\ndata: {value}\n\n")
        })
        .collect();
    let mut response = sse(wire);
    response.headers_mut().insert(
        "x-cma-command-id",
        format!("cmd_{}", agent.trim_start_matches("agt_"))
            .parse()
            .unwrap(),
    );
    response
        .headers_mut()
        .insert("x-cma-response-id", response_id(agent).parse().unwrap());
    response
}

fn gateway(state: &Fixture, headers: &HeaderMap, bytes: &[u8]) -> Response {
    let id = headers["webhook-id"].to_str().unwrap();
    let timestamp = headers["webhook-timestamp"].to_str().unwrap();
    let body = std::str::from_utf8(bytes).unwrap();
    let signature = headers["webhook-signature"]
        .to_str()
        .unwrap()
        .strip_prefix("v1,")
        .unwrap();
    let key = hmac::Key::new(hmac::HMAC_SHA256, "synthetic-secret-value".as_bytes());
    hmac::verify(
        &key,
        format!("{id}.{timestamp}.{body}").as_bytes(),
        &STANDARD.decode(signature).unwrap(),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(value["id"], id);
    let answer = marker(value["data"]["text"].as_str().unwrap());
    assert!(state
        .admissions
        .lock()
        .unwrap()
        .insert(id.to_owned(), (body.to_owned(), answer.to_owned()))
        .is_none());
    let data = json!({"event_id":id,"conversation_id":"conv_gateway","text":answer});
    sse(format!(
        "event: cmaeg.message\ndata: {data}\n\nevent: cmaeg.completed\ndata: {data}\n\n"
    ))
}

fn response_id(agent: &str) -> String {
    format!("resp_{}", agent.trim_start_matches("agt_"))
}

fn response_agent(path: &str) -> String {
    let id = path.split('/').nth(4).unwrap();
    let name = id.trim_start_matches("cmd_").trim_start_matches("resp_");
    format!("agt_{name}")
}

#[tokio::test]
async fn output_page_must_match_admitted_response_in_both_modes() {
    for mode in [Mode::Deterministic, Mode::Funded] {
        let dir = TempDir::new().unwrap();
        let (server, task) = serve(Fault::WrongOutputId).await;
        let config = config(&server.base, dir.path());
        let protocol = Protocol::new(config.clone()).unwrap();
        let intent = protocol
            .intent(job(&config, Check::Responses, mode), now())
            .unwrap();
        let result = first(&protocol, &intent).await;
        assert_eq!(result.reason, Reason::Contract, "{mode:?}");
        assert!(result.evidence.delivery_at.is_some() && result.evidence.receipt_at.is_some());
        assert_eq!(server.admissions.lock().unwrap().len(), 1);
        task.abort();
    }
}

mod publication;
