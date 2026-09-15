use super::{execution::*, fixtures::*};
use axum::http::StatusCode;
use layer2_core::{
    sandbox::{agent_io::*, store::*},
    AgentId,
};
use serde_json::json;
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
async fn queued_agui_waits_for_its_own_turn_and_preserves_durable_wire_ids() {
    let f = fixture(true).await;
    let old = f
        .store
        .get_agent(&AgentId(ID.into()))
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .unwrap();
    let input = json!({"threadId":ID,"runId":"ignored-correlation","messages":[{"role":"user","content":"Next"}]});
    let task = tokio::spawn(
        f.router
            .clone()
            .oneshot(request(AGUI, Some("next"), input.clone())),
    );
    let admitted = admitted(&f).await.remove(0);
    assert!(f.runtime.0.lock().unwrap().is_empty());
    finish(&f, "Earlier output").await;
    let dispatched = execute(&f).await;
    let turn = dispatched.work.dispatch.unwrap().turn_sequence;
    assert!(turn > old);
    finish(&f, "New output").await;
    f.store
        .reconcile_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-cma-command-id"],
        admitted.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        response.headers()["x-cma-response-id"],
        admitted.receipt.response_id.as_ref().unwrap().as_str()
    );
    let wire = body(response).await;
    assert!(!wire.contains("event:"));
    assert!(!wire.contains("Earlier output"));
    let frames = events(&wire);
    assert!(frames
        .iter()
        .all(|event| event["runId"].as_str() == Some(turn.to_string().as_str())));
    assert_eq!(frames.first().unwrap()["type"], "RUN_STARTED");
    assert!(wire.contains("New output"));
    assert_eq!(frames.last().unwrap()["type"], "RUN_FINISHED");
    assert!(wire.contains(&format!("id: {turn}\n")));
    let replay = f
        .router
        .clone()
        .oneshot(request(AGUI, Some("next"), input))
        .await
        .unwrap();
    assert_eq!(body(replay).await, wire);
    assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn legacy_response_replay_keeps_turn_identity_metadata_and_connection_sequence() {
    let f = fixture(false).await;
    let input = json!({"conversation":ID,"stream":true,"input":"First","model":"ignored-model",
        "metadata":{"tag":"kept"},"previous_response_id":null});
    let task = tokio::spawn(f.router.clone().oneshot(request(
        RESPONSES,
        Some("first"),
        input.clone(),
    )));
    let receipt = admitted(&f).await.remove(0).receipt;
    let dispatched = execute(&f).await;
    let turn = dispatched.work.dispatch.unwrap().turn_sequence;
    finish(&f, "Answer").await;
    f.store
        .reconcile_command(&receipt.envelope.command_id)
        .await
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-cma-command-id"],
        receipt.envelope.command_id.as_str()
    );
    let wire = body(response).await;
    let frames = events(&wire);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame["sequence_number"], index);
    }
    let last = frames.last().unwrap();
    assert_eq!(last["type"], "response.completed");
    assert_eq!(last["response"]["id"], format!("resp_{ID}_{turn}"));
    assert_ne!(
        last["response"]["id"],
        receipt.response_id.unwrap().as_str()
    );
    assert_eq!(last["response"]["metadata"], json!({"tag":"kept"}));
    assert_ne!(last["response"]["model"], "ignored-model");
    assert_eq!(
        last["response"]["previous_response_id"],
        serde_json::Value::Null
    );
    let replay = f
        .router
        .clone()
        .oneshot(request(RESPONSES, Some("first"), input))
        .await
        .unwrap();
    assert_eq!(body(replay).await, wire);
    assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_agui_and_v2_share_the_same_raw_key_receipt() {
    let f = fixture(false).await;
    let agui = tokio::spawn(f.router.clone().oneshot(request(
        AGUI,
        Some("cross-interface"),
        json!({"threadId":ID,"messages":[{"role":"user","content":"Same"}]}),
    )));
    let v2 = f
        .router
        .clone()
        .oneshot(request(
            "/api/v2/responses",
            Some("cross-interface"),
            json!({"conversation":ID,"stream":true,"input":"Same"}),
        ))
        .await
        .unwrap();
    assert_eq!(v2.status(), StatusCode::OK);
    let command_id = v2.headers()["x-cma-command-id"].clone();
    let response_id = v2.headers()["x-cma-response-id"].clone();
    drop(v2);
    let records = admitted(&f).await;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].key_sha256,
        command_key_digest("cross-interface").unwrap()
    );
    execute(&f).await;
    finish(&f, "Completed").await;
    let response = tokio::time::timeout(Duration::from_secs(2), agui)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.headers()["x-cma-command-id"], command_id);
    assert_eq!(response.headers()["x-cma-response-id"], response_id);
    assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
    let conflict = f
        .router
        .clone()
        .oneshot(request(
            RESPONSES,
            Some("cross-interface"),
            json!({"conversation":ID,"stream":true,"input":"Same"}),
        ))
        .await
        .unwrap();
    assert_eq!(
        conflict.status(),
        StatusCode::CONFLICT,
        "AfterTurn(None) differs from Append"
    );
    assert!(body(conflict).await.contains("idempotency_key_reused"));
}

#[tokio::test]
async fn disconnect_and_recreated_router_recover_admitted_work_without_resending() {
    let f = fixture(false).await;
    let input = json!({"threadId":ID,"messages":[{"role":"user","content":"Retained"}]});
    let task = tokio::spawn(f.router.clone().oneshot(request(
        AGUI,
        Some("disconnect"),
        input.clone(),
    )));
    let original = admitted(&f).await.remove(0);
    task.abort();
    execute(&f).await;
    finish(&f, "Recovered").await;
    let router = crate::agui::router(f.state.clone());
    let response = router
        .oneshot(request(AGUI, Some("disconnect"), input))
        .await
        .unwrap();
    assert_eq!(
        response.headers()["x-cma-command-id"],
        original.receipt.envelope.command_id.as_str()
    );
    assert!(body(response).await.contains("Recovered"));
    assert_eq!(f.runtime.0.lock().unwrap().len(), 1);
}
