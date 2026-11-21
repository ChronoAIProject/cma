use super::fixtures::*;
use axum::{body::to_bytes, http::Method};
use layer2_core::{
    sandbox::{
        store::AgentStore, AgentLogKind, AgentMessageAssembly, AgentRuntimeUpdate,
        AgentUpdateRecorder, DurableAgentUpdateRecorder,
    },
    AgentId, WorktreeId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn page(router: &axum::Router, path: &str) -> Value {
    let mut read = request(Method::GET, path, "owner-a", None, None);
    read.headers_mut()
        .insert("prefer", "cma-tool-output".parse().unwrap());
    let response = router.clone().oneshot(read).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    serde_json::from_slice(
        &to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn capture_pages(router: &axum::Router, agent: &str) -> Vec<Value> {
    let path = format!("/api/v2/agents/{agent}/events/snapshot?limit=2");
    let mut next = path.clone();
    let mut pages = Vec::new();
    loop {
        let result = page(router, &next).await;
        let cursor = result["next_page_cursor"].as_str().map(str::to_owned);
        pages.push(result);
        match cursor {
            Some(cursor) => next = format!("{path}&page_cursor={cursor}"),
            None => break,
        }
    }
    pages
}

#[tokio::test]
async fn operation_replay_freezes_page_boundaries_and_does_not_admit_commands() {
    let fixture = fixture(true, false).await;
    let recorder =
        DurableAgentUpdateRecorder::with_writer_policy(fixture.store.clone(), task_policy());
    let mut assembly = AgentMessageAssembly::default();
    for number in 1..=3 {
        for update in [
            json!({"sessionUpdate":"tool_call", "toolCallId":format!("call-{number}"), "title":format!("printf record-{number}"), "kind":"execute", "status":"in_progress"}),
            json!({"sessionUpdate":"tool_call_update", "toolCallId":format!("call-{number}"), "status":"completed", "rawOutput":{"formatted_output":format!("record-{number}\n"), "exit_code":0}}),
        ] {
            recorder
                .record_update(
                    &AgentId("agt_fixture".into()),
                    1,
                    &mut assembly,
                    AgentRuntimeUpdate::Said(serde_json::from_value(update).unwrap()),
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
        }
    }
    let first = page(&fixture.router, &format!("{SNAPSHOT}?limit=2")).await;
    let old_head = first["through_cursor"].clone();
    append(&fixture.store, AgentLogKind::PlanSet { entries: vec![] }).await;
    let next = page(
        &fixture.router,
        &format!(
            "{SNAPSHOT}?limit=2&page_cursor={}",
            first["next_page_cursor"].as_str().unwrap()
        ),
    )
    .await;
    assert_eq!(next["through_cursor"], old_head);
    assert_eq!(next["items"][0]["sequence"], 3);
    let before = page(&fixture.router, "/api/v2/agents/agt_fixture/events/head").await;
    let initial = capture_pages(&fixture.router, "agt_fixture").await;
    let reloaded = capture_pages(&fixture.router, "agt_fixture").await;
    assert_eq!(initial, reloaded);
    assert_eq!(
        before,
        page(&fixture.router, "/api/v2/agents/agt_fixture/events/head").await
    );
    let records: Vec<_> = initial
        .iter()
        .flat_map(|page| page["items"].as_array().unwrap())
        .filter(|entry| entry["kind"]["type"] == "tool_started")
        .collect();
    assert_eq!(records.len(), 3);
    for subject in ["outsider", "revoked-reader"] {
        let (status, body) =
            call(&fixture.router, Method::GET, SNAPSHOT, subject, None, None).await;
        assert_ne!(status, 200);
        assert!(!body.to_string().contains("record-"));
    }
}

#[tokio::test]
#[ignore = "requires the pinned native four-command capture and browser completion signal"]
async fn native_operations_survive_public_replay_and_browser_reload() {
    let capture: Value = serde_json::from_slice(
        &std::fs::read(std::env::var("CMA_REPLAY_NATIVE_FILE").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(capture["execution_count"], 4);
    let fixture = fixture(false, false).await;
    let mut agent = fixture
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    agent.id = AgentId("agt_contract".into());
    agent.worktree_id = WorktreeId("wkt_replay".into());
    agent.creation_key = "native-replay-proof".into();
    agent.status = layer2_core::sandbox::AgentStatus::Working;
    agent.conversation.conversation_id = Some("native-replay-conversation".into());
    agent.conversation.last_seq = 4;
    agent.conversation.active_turn = Some(3);
    agent.conversation.runtime_generation = Some(1);
    let agent = crate::rest::agents_tests::insert_agent(
        &fixture.store,
        agent,
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Execute four harmless native commands".into(),
            },
        ],
    )
    .await;
    let recorder =
        DurableAgentUpdateRecorder::with_writer_policy(fixture.store.clone(), task_policy());
    let mut assembly = AgentMessageAssembly::default();
    for update in capture["updates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|update| {
            matches!(
                update["sessionUpdate"].as_str(),
                Some("tool_call" | "tool_call_update" | "plan")
            )
        })
    {
        recorder
            .record_update(
                &agent.id,
                1,
                &mut assembly,
                AgentRuntimeUpdate::Said(serde_json::from_value(update.clone()).unwrap()),
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    let pages = capture_pages(&fixture.router, "agt_contract").await;
    for number in 1..=4 {
        assert!(pages
            .iter()
            .flat_map(|page| page["items"].as_array().unwrap())
            .any(|entry| entry["kind"]["output"]["text"] == format!("native-replay-{number}\n")));
    }
    let head = page(&fixture.router, "/api/v2/agents/agt_contract/events/head").await;
    std::fs::write(
        std::env::var("CMA_REPLAY_PUBLIC_FILE").unwrap(),
        serde_json::to_vec_pretty(&json!({"head":head, "pages":pages})).unwrap(),
    )
    .unwrap();
    let extensions = request(Method::GET, "/", "owner-a", None, None)
        .extensions()
        .clone();
    let service = fixture.router.clone().layer(axum::middleware::from_fn(
        move |mut read: axum::extract::Request, next: axum::middleware::Next| {
            read.extensions_mut().extend(extensions.clone());
            async move { next.run(read).await }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    std::fs::write(
        std::env::var("CMA_REPLAY_ADDRESS_FILE").unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
    )
    .unwrap();
    let release = std::env::var("CMA_REPLAY_PUBLIC_RELEASE").unwrap();
    let completion = release.clone();
    axum::serve(listener, service)
        .with_graceful_shutdown(async move {
            for _ in 0..3600 {
                if std::path::Path::new(&release).exists() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        })
        .await
        .unwrap();
    assert!(
        std::path::Path::new(&completion).exists(),
        "Browser replay proof did not finish"
    );
    assert_eq!(
        head,
        page(&fixture.router, "/api/v2/agents/agt_contract/events/head").await
    );
    assert_eq!(pages, capture_pages(&fixture.router, "agt_contract").await);
}
