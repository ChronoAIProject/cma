use super::{cursor, error::Error, fixtures::*, parse};
use axum::{
    body::to_bytes,
    http::{HeaderMap, Method, StatusCode},
};
use layer2_core::{
    sandbox::{store::AgentStore, AgentLogKind},
    AgentId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[test]
fn cursors_reject_response_sequences_other_agents_noncanonical_and_future_coordinates() {
    let identity = "a".repeat(64);
    assert_eq!(
        cursor::parse_event(&cursor::event(&identity, 17), &identity, 19),
        Ok(17)
    );
    for value in [
        "17".into(),
        format!("ag1.{identity}.017"),
        format!("ag1.{identity}.+17"),
        format!("ag1.{identity}.18446744073709551616"),
        cursor::page(&identity, 19, 17),
    ] {
        assert_eq!(
            cursor::parse_event(&value, &identity, 19),
            Err(Error::Cursor)
        );
    }
    assert_eq!(
        cursor::parse_event(&cursor::event(&"b".repeat(64), 17), &identity, 19),
        Err(Error::Incarnation)
    );
    assert_eq!(
        cursor::parse_event(&cursor::event(&identity, 20), &identity, 19),
        Err(Error::BeyondHead)
    );
    assert_eq!(
        cursor::parse_page(&cursor::page(&identity, 19, 19), &identity, 19),
        Err(Error::Cursor)
    );
}

#[test]
fn queries_reject_duplicates_unknown_fields_and_conflicting_resume_headers() {
    for query in [
        "limit=1&limit=2",
        "limit=201",
        "limit=0",
        "limit=01",
        "owner=me",
        "page_cursor=x&page_cursor=y",
    ] {
        assert!(parse::snapshot(Some(query.into())).is_err());
    }
    for query in [
        "follow=1",
        "after=12",
        "after_cursor=x&after_cursor=y",
        "follow=true&follow=false",
    ] {
        assert!(parse::tail(Some(query.into()), &HeaderMap::new()).is_err());
    }
    let mut headers = HeaderMap::new();
    headers.insert("last-event-id", "x".parse().unwrap());
    assert!(parse::tail(Some("after_cursor=y".into()), &headers).is_err());
    assert!(parse::tail(Some("after_cursor=x".into()), &headers).is_ok());
    headers.append("last-event-id", "x".parse().unwrap());
    assert!(parse::tail(None, &headers).is_err());
}

#[tokio::test]
async fn fixed_snapshot_head_excludes_concurrent_append_then_tail_delivers_it_once() {
    let f = fixture(false, false).await;
    let (status, first) = call(
        &f.router,
        Method::GET,
        &format!("{SNAPSHOT}?limit=1"),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let head = first["through_cursor"].as_str().unwrap();
    append(
        &f.store,
        AgentLogKind::ToolCallStarted {
            call_id: "tool-1".into(),
            title: "Read file".into(),
            kind: "read".into(),
            status: "running".into(),
        },
    )
    .await;
    let (status, second) = call(
        &f.router,
        Method::GET,
        &format!(
            "{SNAPSHOT}?limit=1&page_cursor={}",
            first["next_page_cursor"].as_str().unwrap()
        ),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["through_cursor"], head);
    assert!(second["next_page_cursor"].is_null());
    assert_eq!(second["items"][0]["sequence"], 2);
    let response = f
        .router
        .oneshot(request(
            Method::GET,
            &format!("{EVENTS}?follow=false&after_cursor={head}"),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.matches("event: cma.agent.event").count(), 1);
    let event: Value = serde_json::from_str(
        text.lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        event["kind"],
        json!({"type":"tool_started","call_id":"tool-1","title":"Read file","kind":"read","status":"running"})
    );
    assert_eq!(event["sequence"], 3);
}

#[tokio::test]
async fn binding_is_verified_and_legacy_turns_remain_explicitly_unbound() {
    let f = fixture(true, false).await;
    let (status, page) = call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let turn = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"]["type"] == "turn_started")
        .unwrap();
    assert_eq!(turn["response_binding"]["command_id"], "cmd_control_source");
    assert_eq!(
        turn["response_binding"]["response_id"],
        "resp_control_source"
    );
    assert_eq!(turn["response_binding"]["turn_sequence"], turn["sequence"]);
    for private in [
        "origin_key",
        "runtime_locator",
        "private-source-reference",
        "private-correlation",
        "fixture-custody",
    ] {
        assert!(!page.to_string().contains(private));
    }
    let legacy = fixture(false, false).await;
    append(
        &legacy.store,
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: Some("legacy-key".into()),
        },
    )
    .await;
    let (status, page) = call(&legacy.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["items"][2]["response_binding"].is_null());
    append(
        &legacy.store,
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 4,
            origin_key: Some("command:cmd_missing".into()),
        },
    )
    .await;
    let (status, page) = call(&legacy.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(page["code"], "agent_history_unavailable");
}

#[tokio::test]
async fn eligibility_requires_full_composition_and_flag_off_keeps_recovery_routes() {
    let f = fixture(false, false).await;
    assert!(!super::eligibility::view(&f.state).enabled);
    let mut state = f.state.clone();
    state.ui_flag = true;
    assert!(super::eligibility::view(&state).enabled);
    state.dependencies.controls = false;
    assert!(!super::eligibility::view(&state).enabled);
    state.dependencies.controls = true;
    state.dependencies.responses = false;
    assert!(!super::eligibility::view(&state).enabled);
    state.dependencies.responses = true;
    state.updates = None;
    assert!(!super::eligibility::view(&state).enabled);
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v2/ui/transport",
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let anonymous = axum::http::Request::builder()
        .uri("/api/v2/ui/transport")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        f.router.clone().oneshot(anonymous).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn missing_or_foreign_agents_and_recreated_cursor_are_refused() {
    let f = fixture(false, false).await;
    for (path, subject) in [
        (SNAPSHOT.to_owned(), "foreign"),
        (SNAPSHOT.replace("agt_fixture", "agt_missing"), "owner-a"),
    ] {
        let (status, body) = call(&f.router, Method::GET, &path, subject, None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["code"], "agent_not_found");
    }
    let (_, page) = call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    let mut agent = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.created_at = "2026-09-06T00:00:00Z".into();
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    let (status, body) = call(
        &f.router,
        Method::GET,
        &format!(
            "{EVENTS}?follow=false&after_cursor={}",
            page["through_cursor"].as_str().unwrap()
        ),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "agent_incarnation_changed");
}
