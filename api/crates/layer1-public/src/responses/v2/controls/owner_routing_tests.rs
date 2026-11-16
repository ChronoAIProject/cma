//! Exercise public controls through the service owner-routing port. Removing a
//! service hook makes these tests fail even when an unowned local write succeeds.
mod fixture;
mod queued;
mod refusal;

use axum::http::{Method, StatusCode};
use layer2_core::{
    ids::AgentResponseId,
    sandbox::{agent_io::*, store::PageRequest},
    AgentId,
};
use serde_json::json;
use std::sync::atomic::Ordering;

use fixture::*;

#[tokio::test]
async fn public_control_owner_routing_preserves_exact_target_and_replay() {
    let f = fixture(true).await;
    let body = intent(&f.original, json!({"type":"cancel"})).await;
    let (status, first) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("routed-control"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    let calls = f.route.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let OwnerCommandMutation::TargetedControl(value) = &calls[0] else {
        panic!("wrong routed operation")
    };
    assert_eq!(value.agent_id, AgentId("agt_fixture".into()));
    assert_eq!(
        value.target.response_id,
        AgentResponseId("resp_control_source".into())
    );
    assert_eq!(
        value.target.turn_sequence,
        body["turn_sequence"].as_u64().unwrap()
    );
    assert_eq!(
        value.expected_agent_revision,
        body["expected_agent_revision"].as_u64().unwrap()
    );
    assert_eq!(value.command, AgentCommand::Interrupt);
    assert_eq!(value.idempotency_key, "routed-control");
    let (status, replay) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("routed-control"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
    assert_eq!(replay, first);
    assert_eq!(f.route.calls.lock().unwrap().len(), 2);
    assert_eq!(
        f.store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    let (status, _) = call(&f.router, Method::GET, PATH, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        f.route.calls.lock().unwrap().len(),
        2,
        "observation must stay local"
    );
}

#[tokio::test]
async fn alternate_control_owner_routing_keeps_its_distinct_admission_contract() {
    let f = fixture(true).await;
    let body = intent(&f.original, json!({"type":"cancel"})).await;
    let submission = TargetedControlSubmission {
        agent_id: AgentId("agt_fixture".into()),
        target: TargetedResponse {
            response_id: AgentResponseId("resp_control_source".into()),
            turn_sequence: body["turn_sequence"].as_u64().unwrap(),
        },
        expected_agent_revision: body["expected_agent_revision"].as_u64().unwrap(),
        command: AgentCommand::Interrupt,
        idempotency_key: "direct-control".into(),
    };
    let first = f
        .commands
        .admit_targeted_control(
            &context("owner-a"),
            "fixture-credential",
            CommandSubmissionSource::Api,
            submission.clone(),
        )
        .await
        .unwrap();
    let replay = f
        .commands
        .admit_targeted_control(
            &context("owner-a"),
            "fixture-credential",
            CommandSubmissionSource::Api,
            submission,
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    let calls = f.route.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert!(calls
        .iter()
        .all(|value| matches!(value, OwnerCommandMutation::DirectTargetedControl(_))));
}
