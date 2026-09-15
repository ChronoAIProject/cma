use super::fixtures::*;
use axum::http::{Method, StatusCode};
use layer2_core::{
    sandbox::{
        agent_io::AgentCommandWorkerStore,
        store::{AgentStore, PageRequest},
    },
    AgentId,
};
use serde_json::json;

#[tokio::test]
async fn response_control_proves_absent_stale_intent_but_preserves_uncertain_conflicts() {
    let f = fixture(true).await;
    let body = intent(&f, json!({"type":"cancel"})).await;
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let previous = agent.revision;
    agent.revision += 1;
    agent.conversation.active_turn = Some(999);
    f.store.save_agent(previous, agent).await.unwrap();
    for _ in 0..2 {
        let (status, problem) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("original"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["error"]["code"], "response_control_not_admitted");
    }
    for (revision, turn) in [(previous + 1, 4), (previous + 2, 4), (previous, 999)] {
        let mut changed = body.clone();
        changed["expected_agent_revision"] = json!(revision);
        changed["turn_sequence"] = json!(turn);
        let (status, problem) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("other"),
            Some(changed),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["error"]["code"], "response_control_conflict");
    }
    let (status, problem) = call(
        &f.router,
        Method::POST,
        PATH,
        "foreign",
        Some("original"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["error"]["code"], "response_not_found");
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
