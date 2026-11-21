use super::fixtures::*;
use axum::http::{Method, StatusCode};
use layer2_core::{
    ids::AgentCommandId,
    sandbox::{
        agent_io::*,
        store::{AgentStore, PageRequest},
    },
    AgentId, RuntimeCapabilities, RuntimeCapability,
};
use serde_json::{json, Value};

#[tokio::test]
async fn response_controls_recover_exact_intent_and_command_status_without_private_fields() {
    let f = fixture(true).await;
    let body = intent(&f, json!({"type":"cancel"})).await;
    let (status, original) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("cancel-key"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{original}");
    assert_eq!(original["control"]["status"], "queued");
    assert_eq!(original["control"]["receipt"]["response_id"], Value::Null);
    let command = original["control"]["receipt"]["command_id"]
        .as_str()
        .unwrap();
    let (status, status_view) = call(
        &f.router,
        Method::GET,
        &format!("/api/v2/commands/{command}"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_view, original["control"]);
    let encoded = original.to_string();
    for private in [
        "owner",
        "actor",
        "source",
        "authority",
        "credential",
        "runtime_locator",
        "private-correlation",
        "Private source input",
    ] {
        assert!(
            !encoded.contains(&format!("\"{private}\"")),
            "leaked {private}"
        );
    }
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let previous = agent.revision;
    agent.revision += 1;
    agent.conversation.active_turn = Some(999);
    agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
    f.store.save_agent(previous, agent).await.unwrap();
    let (status, retry) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("cancel-key"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{retry}");
    assert_eq!(retry, original);
    let mut changed = body;
    changed["expected_agent_revision"] = json!(previous + 1);
    let (status, conflict) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("cancel-key"),
        Some(changed),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "agent_command_key_conflict");
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
}

#[tokio::test]
async fn response_control_observation_never_invents_a_turn_and_cross_agent_reads_are_hidden() {
    let f = fixture(false).await;
    let (status, observed) = call(&f.router, Method::GET, PATH, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(observed["target"], Value::Null);
    assert_eq!(observed["target_active"], false);
    for path in [
        PATH.to_owned(),
        PATH.replace("agt_fixture", "agt_missing"),
        format!("/api/v2/commands/{SOURCE}"),
        "/api/v2/commands/cmd_missing".into(),
    ] {
        let (status, body) = call(&f.router, Method::GET, &path, "unrelated", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "response_not_found");
    }
    let (status, _) = call(
        &f.router,
        Method::GET,
        &PATH.replace("agt_fixture", "agt_missing"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn response_controls_distinguish_unsupported_operations_from_stale_targets() {
    let f = fixture(true).await;
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let previous = agent.revision;
    agent.capabilities = RuntimeCapabilities::new([RuntimeCapability::Conversation]);
    agent.revision += 1;
    f.store.save_agent(previous, agent).await.unwrap();
    let mut body = intent(&f, json!({"type":"cancel"})).await;
    let (status, unsupported) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("unsupported"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(unsupported["error"]["code"], "capability_unsupported");
    body["turn_sequence"] = json!(999);
    let (status, stale) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("stale"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["error"]["code"], "response_control_conflict");
}

#[tokio::test]
async fn response_controls_accept_steer_and_exact_pending_approval() {
    let f = fixture(true).await;
    let body = intent(&f, json!({"type":"steer","text":"Look at this first"})).await;
    assert_eq!(
        call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("steer"),
            Some(body)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    pending_approval(&f, "approval-exact").await;
    let mut body = intent(
        &f,
        json!({"type":"resolve_approval","approval_id":"approval-exact","decision":"allow"}),
    )
    .await;
    let (status, accepted) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("approval"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    body["command"]["approval_id"] = json!("approval-other");
    let (status, wrong) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("approval-other"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(wrong["error"]["code"], "response_control_conflict");
    let command_id = AgentCommandId(
        accepted["control"]["receipt"]["command_id"]
            .as_str()
            .unwrap()
            .into(),
    );
    assert_eq!(
        f.store
            .get_command(&command_id)
            .await
            .unwrap()
            .unwrap()
            .receipt
            .envelope
            .actor,
        context("owner-a").actor
    );
}

#[tokio::test]
async fn completed_cancel_command_does_not_claim_response_cancellation() {
    let f = fixture(true).await;
    let body = intent(&f, json!({"type":"cancel"})).await;
    assert_eq!(
        call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("cancel"),
            Some(body)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let id = execute_next(&f).await;
    let (status, delivered) = call(
        &f.router,
        Method::GET,
        &format!("/api/v2/commands/{id}"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(delivered["status"], "completed");
    assert_eq!(delivered["delivery"], "write_confirmed");
    let (_, observed) = call(&f.router, Method::GET, PATH, "owner-a", None, None).await;
    assert_eq!(observed["response"]["status"], "in_progress");
    assert_eq!(observed["target_active"], true);
}
