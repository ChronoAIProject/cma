use axum::http::{Method, StatusCode};
use layer2_core::sandbox::{
    AgentLogKind, AgentMessageAssembly, AgentRuntimePromptResponse, AgentRuntimeUpdate,
    AgentStatus, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use serde_json::{json, Value};

use crate::conversation::command_test_support::compose;
use crate::responses::v2::controls::fixtures::request;
use crate::rest::agents_tests::{conversation_fixture, insert_agent, response, seeded_agent};

fn native_results() -> Value {
    if let Ok(path) = std::env::var("CMA_COMPLETION_NATIVE_PROOF") {
        let evidence: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(evidence["provider"], "codex-cli 0.147.0");
        assert_eq!(
            evidence["bridgeSha256"],
            "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c"
        );
        assert_eq!(
            evidence["statuses"],
            json!(["completed", "interrupted", "completed"])
        );
        return evidence;
    }
    json!({"completed":{"stopReason":"end_turn"},"interrupted":{"stopReason":"cancelled"}})
}

fn frames(wire: &str) -> Vec<Value> {
    wire.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn completion_outcomes_keep_recorded_time_and_success_distinct_on_both_public_transports() {
    let native = native_results();
    let mut proof = serde_json::Map::new();
    for (result, outcome, kind) in [
        ("completed", "complete", "turn_completed"),
        ("interrupted", "interrupt", "turn_cancelled"),
    ] {
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
                    content: "Observe the actual native terminal outcome.".into(),
                },
            ],
        )
        .await;
        let state = compose(state, store.clone());
        let router = super::router(state.clone()).merge(crate::agent_v2::router(
            crate::agent_v2::AgentV2State::new(
                state.reads.clone(),
                state.commands.clone().unwrap(),
            ),
        ));
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
        let active = String::from_utf8(body.to_vec()).unwrap();
        assert!(!frames(&active)
            .iter()
            .any(|frame| frame["type"] == "RUN_FINISHED"));
        let recorder = DurableAgentUpdateRecorder::new(store);
        let native_result: AgentRuntimePromptResponse =
            serde_json::from_value(native[result].clone()).unwrap();
        recorder
            .record_update(
                &agent.id,
                1,
                &mut AgentMessageAssembly::default(),
                AgentRuntimeUpdate::TurnEnded(native_result),
                "2026-09-05T00:04:00Z".into(),
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
        let events = frames(&wire);
        let started = events
            .iter()
            .find(|frame| frame["type"] == "RUN_STARTED")
            .unwrap();
        let ended = events
            .iter()
            .find(|frame| frame["type"] == "RUN_FINISHED")
            .unwrap();
        assert_eq!(started["timestamp"], "2026-09-05T00:00:00Z");
        assert_eq!(ended["timestamp"], "2026-09-05T00:04:00Z");
        assert_eq!(ended["outcome"]["type"], outcome);
        assert_eq!(ended["runId"], started["runId"]);
        let (status, body) = response(
            &router,
            request(
                Method::GET,
                "/api/v2/agents/agt_contract/events/snapshot",
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let snapshot: Value = serde_json::from_slice(&body).unwrap();
        let items = snapshot["items"].as_array().unwrap();
        let started = items
            .iter()
            .find(|item| item["kind"]["type"] == "turn_started")
            .unwrap();
        let ended = items
            .iter()
            .find(|item| item["kind"]["type"] == kind)
            .unwrap();
        assert_eq!(started["appended_at"], "2026-09-05T00:00:00Z");
        assert_eq!(ended["appended_at"], "2026-09-05T00:04:00Z");
        assert_eq!(
            ended["kind"]["turn_sequence"],
            started["kind"]["turn_sequence"]
        );
        let (status, body) = response(
            &router,
            request(
                Method::GET,
                "/api/v1/agui/events?thread_id=agt_contract&starting_after=4&follow=false",
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let tail = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(
            frames(&tail)
                .iter()
                .filter(|frame| frame["type"] == "RUN_FINISHED")
                .count(),
            1
        );
        assert!(!frames(&tail)
            .iter()
            .any(|frame| frame["type"] == "RUN_STARTED"));
        proof.insert(
            result.into(),
            json!({"active":active,"agui":wire,"tail":tail,"responses":snapshot}),
        );
    }
    if let Ok(path) = std::env::var("CMA_COMPLETION_PUBLIC_PROOF") {
        std::fs::write(path, serde_json::to_vec_pretty(&proof).unwrap()).unwrap();
    }
}
