use super::PREFERENCE;
use crate::agent_v2::{fixtures::*, router};
use axum::{body::to_bytes, http::Method};
use layer2_core::{
    ids::{AgentCommandId, AgentId},
    sandbox::{
        agent_io::*,
        store::*,
        turn_timeout::{approval::*, *},
        *,
    },
};
use serde_json::{json, Value};
use std::time::{Duration, UNIX_EPOCH};
use tower::ServiceExt;

fn approval_headers(headers: &axum::http::HeaderMap) -> Value {
    json!({
        "Content-Type": headers["content-type"].to_str().unwrap(),
        "Preference-Applied": headers["preference-applied"].to_str().unwrap(),
        "CMA-Approval-Review-Origin": headers["cma-approval-review-origin"].to_str().unwrap(),
        "CMA-External-Approval-Action": headers["cma-external-approval-action"].to_str().unwrap()
    })
}

async fn read(f: &Fixture, path: &str) -> (axum::http::HeaderMap, Vec<u8>) {
    let mut request = request(Method::GET, path, "owner-a", None, None);
    request
        .headers_mut()
        .insert("prefer", PREFERENCE.parse().unwrap());
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["preference-applied"], PREFERENCE);
    assert_eq!(headers["cma-external-approval-action"], "review");
    assert_eq!(
        headers["cma-approval-review-origin"],
        "https://identity.example.test"
    );
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (headers, body.to_vec())
}

#[tokio::test]
async fn committed_approval_timeout_projects_the_same_order_in_snapshot_and_replayed_sse() {
    for (buffered, fraction_ms) in [false, true]
        .into_iter()
        .flat_map(|buffered| [0, 1, 10, 100].map(|fraction| (buffered, fraction)))
    {
        let mut f = fixture(true, false).await;
        f.router = router(
            f.state
                .clone()
                .with_single_fence_pages(buffered)
                .with_external_approval_review_origin("https://identity.example.test"),
        );
        let id = AgentId("agt_fixture".into());
        let command_id = AgentCommandId(crate::responses::v2::controls::fixtures::SOURCE.into());
        let command = f.store.reconcile_command(&command_id).await.unwrap();
        let agent = f.store.get_agent(&id).await.unwrap().unwrap();
        let dispatch = command.work.dispatch.as_ref().unwrap();
        let native = agent.conversation.conversation_id.as_ref().unwrap();
        let at = layer2_core::now_rfc3339();
        let now_ms = layer2_core::seconds_from_rfc3339(&at).unwrap() as i64 * 1_000 + fraction_ms;
        let admitted = TurnTimeout::admit_approval(
            TurnTimeoutBinding {
                agent_id: id.clone(),
                creation_key: agent.creation_key.clone(),
                creation_fingerprint: agent.creation_fingerprint.clone(),
                command_id: command_id.clone(),
                turn_sequence: dispatch.turn_sequence,
                operation_sequence: dispatch.operation_sequence,
                runtime_generation: dispatch.runtime_generation,
                runtime_boot: "b".repeat(32),
                conversation_id: native.clone(),
                prompt_id: layer3_adapter::PromptIdentity::new(
                    native,
                    dispatch.runtime_generation,
                    dispatch.turn_sequence,
                )
                .as_str()
                .into(),
            },
            uuid::Uuid::new_v4().to_string(),
            ApprovalLimits::default(),
            ApprovalClockBounds {
                offset_min_ms: 0,
                offset_max_ms: 0,
                valid_until_ms: now_ms + 901_000,
            },
            now_ms,
        )
        .unwrap();
        let operation = ApprovalOperationBinding {
            operation_id: "private-provider-operation".into(),
            model_attempt_id: "private-model-attempt".into(),
            provider: "nyxid".into(),
            authority_digest: "c".repeat(64),
        };
        let registered = admitted
            .register_approval_operation(admitted.revision(), operation.clone(), now_ms)
            .unwrap();
        let waiting = registered
            .observe_pending_approval(
                registered.revision(),
                &PendingApprovalReceipt {
                    binding: operation,
                    review_id: "private-review-id".into(),
                    revision: 30,
                    created_at_ms: now_ms,
                    expires_at_ms: now_ms + 901_000,
                },
                now_ms,
            )
            .unwrap();
        let requested = waiting.approval_events_since(&registered).unwrap();
        assert_eq!(requested.len(), 1);
        for kind in requested {
            append(&f.store, kind).await;
        }
        append(
            &f.store,
            AgentLogKind::StatusChanged {
                from: agent.status,
                to: AgentStatus::WaitingApproval,
            },
        )
        .await;
        let mut pending_agent = f.store.get_agent(&id).await.unwrap().unwrap();
        pending_agent
            .transition(AgentStatus::WaitingApproval, at.clone(), None)
            .unwrap();
        pending_agent.conversation.turn_timeout = Some(waiting.clone());
        // Admission and authenticated receipt ingestion are separate contracts.
        // Start this publication test at an already persisted pending policy.
        f.store.seed_turn_policy_fixture(pending_agent);
        let command = f.store.reconcile_command(&command_id).await.unwrap();
        assert_eq!(command.status, AgentResponseStatus::WaitingAction);
        assert!(f
            .store
            .pending_agent_approvals(&id)
            .await
            .unwrap()
            .is_empty());

        let (headers, body) = read(&f, SNAPSHOT).await;
        let pending: Value = serde_json::from_slice(&body).unwrap();
        let (head_headers, body) = read(&f, &format!("{EVENTS}/head")).await;
        let head: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(head["through_cursor"], pending["through_cursor"]);
        let rows = pending["items"].as_array().unwrap();
        let start = rows
            .iter()
            .find(|row| row["kind"]["type"] == "turn_started")
            .unwrap();
        assert_eq!(start["response_binding"]["command_id"], command_id.as_str());
        assert_eq!(
            start["response_binding"]["response_id"],
            "resp_control_source"
        );
        assert_eq!(
            start["response_binding"]["turn_sequence"],
            dispatch.turn_sequence
        );
        assert_eq!(head["active_turn"], *start);
        let request = rows
            .iter()
            .find(|row| row["kind"]["type"] == "external_approval_requested")
            .unwrap();
        assert!(request["response_binding"].is_null());
        assert_eq!(request["kind"]["provider"], "nyxid");
        assert_eq!(request["kind"]["revision"], waiting.revision());
        assert_eq!(request["kind"]["turn_sequence"], dispatch.turn_sequence);
        assert_eq!(
            request["kind"]["approve_url"],
            "https://identity.example.test/assistant/approvals"
        );

        DurableAgentUpdateRecorder::new(f.store.clone())
            .record_update(
                &id,
                dispatch.runtime_generation,
                &mut AgentMessageAssembly::default(),
                AgentRuntimeUpdate::Said(
                    serde_json::from_value(json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "Synthetic partial answer"}
                    }))
                    .unwrap(),
                ),
                at,
            )
            .await
            .unwrap();
        // The publication clock uses whole seconds; settle at the first
        // observation at or after the exact millisecond policy deadline.
        let terminal_second = (waiting.deadline_at_ms() + 999) / 1_000;
        let terminal_at =
            layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(terminal_second as u64));
        let snapshot = f.store.agent_message_snapshot(&id).await.unwrap().unwrap();
        assert!(waiting.binding().matches_active_turn(&snapshot.agent));
        assert!(snapshot
            .checkpoint
            .as_ref()
            .is_some_and(|value| value.matches(&snapshot.agent)));
        assert_eq!(
            snapshot.agent.conversation.turn_timeout.as_ref(),
            Some(&waiting)
        );
        waiting
            .expire(waiting.revision(), now_ms + 300_000)
            .unwrap();
        let settlement =
            TurnTimeoutSettlement::prepare(snapshot, command, waiting.revision(), terminal_at)
                .unwrap();
        f.store.commit_turn_timeout(&settlement).await.unwrap();
        f.store.commit_turn_timeout(&settlement).await.unwrap();
        let (terminal_headers, body) = read(&f, SNAPSHOT).await;
        let terminal: Value = serde_json::from_slice(&body).unwrap();
        let prefix = pending["items"].as_array().unwrap();
        let all = terminal["items"].as_array().unwrap();
        assert_eq!(&all[..prefix.len()], prefix);
        assert_eq!(all[prefix.len()]["kind"]["type"], "delivery_changed");
        assert_eq!(
            settlement.snapshot().agent.conversation.last_seq as usize,
            prefix.len() + 1
        );
        let suffix = &all[prefix.len() + 1..];
        assert_eq!(
            suffix
                .iter()
                .map(|row| row["kind"]["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "message",
                "external_approval_resolved",
                "turn_failed",
                "status_changed"
            ]
        );
        assert_eq!(suffix[0]["kind"]["content"], "Synthetic partial answer");
        assert_eq!(
            suffix[1]["kind"],
            json!({
                "type": "external_approval_resolved",
                "approval_id": request["kind"]["approval_id"],
                "provider": "nyxid", "turn_sequence": dispatch.turn_sequence,
                "revision": waiting.revision() + 1, "resolution": "timed_out"
            })
        );
        assert_eq!(
            suffix[2]["kind"]["failure"],
            serde_json::to_value(TurnTimeoutCause::ApprovalBudget.failure()).unwrap()
        );
        assert_eq!(suffix[2]["kind"]["turn_sequence"], dispatch.turn_sequence);
        assert!(suffix.iter().all(|row| row["response_binding"].is_null()));
        let failed = f.store.get_command(&command_id).await.unwrap().unwrap();
        assert_eq!(failed.status, AgentResponseStatus::Failed);
        assert!(f
            .store
            .get_agent(&id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .turn_timeout
            .unwrap()
            .blocks_dispatch());

        let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
        for update in [
            AgentRuntimeUpdate::Said(
                serde_json::from_value(json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "Late output must not reopen this turn"}
                }))
                .unwrap(),
            ),
            AgentRuntimeUpdate::FailedForPrompt {
                prompt_id: layer3_adapter::PromptIdentity::new(
                    native,
                    dispatch.runtime_generation,
                    dispatch.turn_sequence,
                ),
                message: "Late runtime failure".into(),
                capability: None,
            },
        ] {
            recorder
                .record_update(
                    &id,
                    dispatch.runtime_generation,
                    &mut AgentMessageAssembly::default(),
                    update,
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
        }
        let (_, after_late_output) = read(&f, SNAPSHOT).await;
        assert_eq!(
            serde_json::from_slice::<Value>(&after_late_output).unwrap(),
            terminal
        );

        let (_, wire) = read(&f, &format!("{EVENTS}?follow=false")).await;
        let wire = String::from_utf8(wire).unwrap();
        let events: Vec<Value> = wire
            .split("\n\n")
            .filter(|frame| frame.lines().any(|line| line == "event: cma.agent.event"))
            .map(|frame| {
                let data = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .unwrap();
                let event: Value = serde_json::from_str(data).unwrap();
                let cursor = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("id: "))
                    .unwrap();
                assert_eq!(event["cursor"], cursor);
                assert_eq!(event["agent_id"], terminal["agent_id"]);
                assert_eq!(event["incarnation"], terminal["incarnation"]);
                event
            })
            .collect();
        assert_eq!(events.len(), all.len());
        for (event, row) in events.iter().zip(all) {
            let mut item = event.clone();
            item.as_object_mut().unwrap().remove("agent_id");
            item.as_object_mut().unwrap().remove("incarnation");
            assert_eq!(&item, row);
        }
        for private in [
            "private-provider-operation",
            "private-model-attempt",
            "private-review-id",
            &"c".repeat(64),
            &"b".repeat(32),
        ] {
            assert!(!wire.contains(private));
        }
        let after = pending["through_cursor"].as_str().unwrap();
        let (stream_headers, resumed_wire) =
            read(&f, &format!("{EVENTS}?follow=false&after_cursor={after}")).await;
        println!(
            "EXTERNAL_APPROVAL_PRODUCER_FIXTURE {}",
            json!({
                "buffered": buffered,
                "fraction_ms": fraction_ms,
                "response_headers": approval_headers(&headers),
                "head_headers": approval_headers(&head_headers),
                "terminal_headers": approval_headers(&terminal_headers),
                "stream_headers": approval_headers(&stream_headers),
                "waiting_snapshot": pending,
                "waiting_head": head,
                "terminal_snapshot": terminal,
                "resumed_stream": String::from_utf8(resumed_wire).unwrap(),
                "replayed_events": events
            })
        );
    }
}
