use super::fixtures::*;
use axum::http::{Method, StatusCode};
use layer2_core::sandbox::AgentLogKind;

const STATE: &str = "/api/v2/agents/agt_fixture/state";

#[tokio::test]
async fn current_controls_survive_history_windows_and_approval_pages_fence_head_changes() {
    let f = fixture(true, false).await;
    append(
        &f.store,
        AgentLogKind::AvailableCommandsSet { commands: vec![] },
    )
    .await;
    append(&f.store, AgentLogKind::PlanSet { entries: vec![] }).await;
    append(&f.store, AgentLogKind::ConversationGap { frames: 2 }).await;
    for number in 0..102 {
        append(
            &f.store,
            AgentLogKind::ApprovalRequested {
                approval_id: format!("approval-{number}"),
                action: Some("fixture".into()),
                detail: None,
                runtime_locator: None,
            },
        )
        .await;
    }
    append(
        &f.store,
        AgentLogKind::ApprovalResolved {
            approval_id: "approval-0".into(),
            decision: "approved".into(),
        },
    )
    .await;
    let (status, first) = call(&f.router, Method::GET, STATE, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["approvals"].as_array().unwrap().len(), 100);
    let context = first["context"].as_array().unwrap();
    assert!(context
        .iter()
        .any(|item| item["kind"]["type"] == "turn_started"
            && item["response_binding"]["command_id"] == "cmd_control_source"));
    assert!(context
        .iter()
        .any(|item| item["kind"]["type"] == "available_commands_set"));
    assert!(context
        .iter()
        .any(|item| item["kind"]["type"] == "plan_set"));
    assert!(context
        .iter()
        .any(|item| item["kind"]["type"] == "history_gap"));
    let next = format!(
        "{STATE}?through_cursor={}&after_approval_cursor={}",
        first["through_cursor"].as_str().unwrap(),
        first["next_approval_cursor"].as_str().unwrap()
    );
    let (status, last) = call(&f.router, Method::GET, &next, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last["approvals"].as_array().unwrap().len(), 1);
    assert!(last["next_approval_cursor"].is_null());
    append(
        &f.store,
        AgentLogKind::ApprovalResolved {
            approval_id: "approval-1".into(),
            decision: "denied".into(),
        },
    )
    .await;
    let (status, changed) = call(&f.router, Method::GET, &next, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(changed["code"], "agent_state_changed");
    assert_eq!(
        call(&f.router, Method::GET, STATE, "owner-b", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.router,
            Method::GET,
            &format!("{STATE}?after_approval_cursor=invalid"),
            "owner-a",
            None,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
