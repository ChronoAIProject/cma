use super::{cursor, fixtures::*};
use axum::http::{Method, StatusCode};
use layer2_core::{
    sandbox::{store::AgentStore, AgentLogKind},
    AgentId,
};

const HISTORY: &str = "/api/v2/agents/agt_fixture/history";

#[tokio::test]
async fn history_windows_keep_mid_turn_context_and_original_frozen_cursors() {
    let f = fixture(true, false).await;
    append(&f.store, AgentLogKind::ConversationGap { frames: 2 }).await;
    for _ in 0..8 {
        append(
            &f.store,
            AgentLogKind::AgentMessage {
                content: "fixture output".into(),
            },
        )
        .await;
    }
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let (status, page) = call(
        &f.router,
        Method::GET,
        &format!("{HISTORY}?limit=3"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let end = agent.conversation.last_seq;
    let identity = page["incarnation"].as_str().unwrap();
    assert_eq!(page["object"], "cma.agent.history_window");
    assert_eq!(page["through_cursor"], cursor::event(identity, end));
    assert_eq!(page["start_cursor"], cursor::event(identity, end - 3));
    assert_eq!(page["previous_cursor"], page["start_cursor"]);
    assert_eq!(
        page["context"]["sequence"],
        agent.conversation.active_turn.unwrap()
    );
    assert_eq!(
        page["context"]["response_binding"]["command_id"],
        "cmd_control_source"
    );
    assert_eq!(page["gap"]["kind"]["type"], "history_gap");
    assert_eq!(page["gap"]["kind"]["frames"], 2);
    assert_eq!(page["items"].as_array().unwrap().len(), 3);
    append(
        &f.store,
        AgentLogKind::AgentMessage {
            content: "later append".into(),
        },
    )
    .await;
    let (status, frozen) = call(
        &f.router,
        Method::GET,
        &format!(
            "{HISTORY}?limit=3&through_cursor={}",
            page["through_cursor"].as_str().unwrap()
        ),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(frozen, page);
    let (status, previous) = call(
        &f.router,
        Method::GET,
        &format!(
            "{HISTORY}?limit=3&through_cursor={}",
            page["previous_cursor"].as_str().unwrap()
        ),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(previous["through_cursor"], page["start_cursor"]);
    assert_eq!(
        previous["items"].as_array().unwrap().last().unwrap()["sequence"],
        end - 3
    );
}

#[tokio::test]
async fn history_byte_bound_keeps_the_latest_suffix_and_requires_current_authority() {
    let f = fixture(true, false).await;
    for _ in 0..20 {
        append(
            &f.store,
            AgentLogKind::AgentMessage {
                content: "x".repeat(256 * 1024),
            },
        )
        .await;
    }
    let agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let (status, page) = call(
        &f.router,
        Method::GET,
        &format!("{HISTORY}?limit=100"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "unexpected history status");
    assert!(serde_json::to_vec(&page).unwrap().len() < 4 * 1024 * 1024);
    let rows = page["items"].as_array().unwrap();
    assert!(rows.len() < 20);
    assert_eq!(
        rows.last().unwrap()["sequence"],
        agent.conversation.last_seq
    );
    assert!(page["previous_cursor"].is_string());
    assert_eq!(
        call(&f.router, Method::GET, HISTORY, "owner-b", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    for query in [
        "limit=201",
        "limit=1&limit=2",
        "through_cursor=invalid",
        "page_cursor=unsupported",
    ] {
        assert_eq!(
            call(
                &f.router,
                Method::GET,
                &format!("{HISTORY}?{query}"),
                "owner-a",
                None,
                None
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}
