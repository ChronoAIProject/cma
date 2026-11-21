use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{
    AgentLogKind, AgentMessageAssembly, AgentRuntimePromptResponse, AgentRuntimeTurnStopReason,
    AgentRuntimeUpdate, AgentStatus, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use serde_json::{json, Value};

use crate::rest::agents_tests::{
    conversation_fixture, insert_agent, request, response, seeded_agent,
};

fn updates() -> Vec<Value> {
    if let Ok(path) = std::env::var("CMA_PLAN_NATIVE_PROOF") {
        let evidence: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(evidence["provider"], "codex-cli 0.147.0");
        assert_eq!(
            evidence["bridgeSha256"],
            "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c"
        );
        assert_eq!(evidence["noPlanTurn"], true);
        return evidence["plans"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| frame["update"].clone())
            .collect();
    }
    vec![
        json!({"sessionUpdate":"plan","entries":[
            {"content":"Read the instructions","status":"in_progress","priority":"medium"},
            {"content":"Implement the change","status":"pending","priority":"medium"},
            {"content":"Verify the result","status":"pending","priority":"medium"}
        ]}),
        json!({"sessionUpdate":"plan","entries":[
            {"content":"Read the instructions","status":"completed","priority":"medium"},
            {"content":"Implement the change","status":"in_progress","priority":"medium"},
            {"content":"Verify the result","status":"pending","priority":"medium"}
        ]}),
    ]
}

fn frames(wire: &str) -> Vec<Value> {
    wire.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn recorded_plan_changes_survive_public_stream_reconnect_without_synthetic_completion() {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_contract", AgentStatus::Working, 4, Some(3));
    insert_agent(
        &store,
        agent.clone(),
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Report a three-step plan and update its progress.".into(),
            },
        ],
    )
    .await;
    let recorder = DurableAgentUpdateRecorder::new(store);
    let mut assembly = AgentMessageAssembly::default();
    let router = super::router(state);
    let reports = updates();
    assert_eq!(reports.len(), 2);
    let mut initial_wire = String::new();
    let mut changed_wire = String::new();
    for (index, report) in reports.into_iter().enumerate() {
        recorder
            .record_update(
                &agent.id,
                1,
                &mut assembly,
                AgentRuntimeUpdate::Said(serde_json::from_value(report).unwrap()),
                "2026-11-03T00:00:01Z".into(),
            )
            .await
            .unwrap();
        let after = if index == 0 { 0 } else { 5 };
        let (status, body) = response(&router, request(Method::GET,
            &format!("/api/v1/agui/events?thread_id=agt_contract&starting_after={after}&follow=false"),
            "owner-a", None, None)).await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let wire = String::from_utf8(body.to_vec()).unwrap();
        let values = frames(&wire);
        let plans: Vec<_> = values
            .iter()
            .filter(|frame| frame["delta"][0]["path"] == "/plan")
            .collect();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0]["runId"], "3");
        assert_eq!(
            plans[0]["delta"][0]["value"][0]["status"],
            if index == 0 {
                "in_progress"
            } else {
                "completed"
            }
        );
        assert_eq!(
            plans[0]["delta"][0]["value"][1]["title"],
            "Implement the change"
        );
        assert_eq!(plans[0]["delta"][0]["value"][2]["status"], "pending");
        if index == 0 {
            initial_wire = wire;
        } else {
            changed_wire = format!("{initial_wire}{wire}");
        }
    }
    recorder
        .record_update(
            &agent.id,
            1,
            &mut assembly,
            AgentRuntimeUpdate::TurnEnded(AgentRuntimePromptResponse::new(
                AgentRuntimeTurnStopReason::EndTurn,
            )),
            "2026-11-03T00:00:02Z".into(),
        )
        .await
        .unwrap();
    let (status, body) = response(
        &router,
        request(
            Method::GET,
            "/api/v1/agui/events?thread_id=agt_contract&follow=false",
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let wire = String::from_utf8(body.to_vec()).unwrap();
    let replay = frames(&wire);
    assert_eq!(
        replay
            .iter()
            .filter(|frame| frame["delta"][0]["path"] == "/plan")
            .count(),
        2
    );
    assert!(replay
        .iter()
        .any(|frame| frame["type"] == "RUN_FINISHED" && frame["outcome"]["type"] == "complete"));
    if let Ok(path) = std::env::var("CMA_PLAN_PUBLIC_PROOF") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"initial": initial_wire, "changed": changed_wire, "replay": wire}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}
