use super::*;
use crate::agent_v2::{fixtures::*, router};
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer2_core::sandbox::store::AgentConversationStore;
use layer2_core::sandbox::{AgentLogKind, ExternalApprovalResolution};
use serde_json::{json, Value};
use tower::ServiceExt;

fn pending() -> AgentLogKind {
    AgentLogKind::ExternalApprovalRequested {
        approval_id: "ea_11111111111111111111111111111111".into(),
        provider: "nyxid".into(),
        turn_sequence: 3,
        revision: 1,
        waiting_since: "2026-01-01T00:00:01.000Z".into(),
        deadline_at: "2026-01-01T00:05:01.000Z".into(),
    }
}

#[test]
fn only_a_configured_https_origin_can_supply_the_fixed_inbox() {
    assert_eq!(
        ReviewOrigin::configured("https://identity.example.test/")
            .0
            .as_deref(),
        Some("https://identity.example.test")
    );
    for origin in [
        "",
        "http://identity.example.test",
        "https://actor@identity.example.test",
        "https://identity.example.test/approvals",
        "https://identity.example.test/?token=x",
        "https://identity.example.test/#fragment",
        "https:\\identity.example.test",
        " https://identity.example.test",
    ] {
        assert!(ReviewOrigin::configured(origin).0.is_none(), "{origin}");
    }
}

#[test]
fn shared_snapshot_and_sse_corpus_round_trips_through_the_actual_rich_envelopes() {
    use crate::agent_v2::shapes::{DurableEvent, EventPage, ResponseBinding};
    fn item(value: &Value) -> EventItem {
        let binding = &value["response_binding"];
        EventItem {
            retained_output: None,
            message_command_id: None,
            cursor: value["cursor"].as_str().unwrap().into(),
            sequence: value["sequence"].as_u64().unwrap(),
            appended_at: value["appended_at"].as_str().unwrap().into(),
            kind: serde_json::from_value(value["kind"].clone()).unwrap(),
            response_binding: (!binding.is_null()).then(|| ResponseBinding {
                command_id: binding["command_id"].as_str().unwrap().into(),
                response_id: binding["response_id"].as_str().unwrap().into(),
                turn_sequence: binding["turn_sequence"].as_u64().unwrap(),
            }),
        }
    }
    let corpus: Value = serde_json::from_str(include_str!("fixtures.json")).unwrap();
    for scenario in ["waiting_snapshot", "resolved_snapshot", "terminal_snapshot"] {
        let expected = &corpus[scenario];
        let page = EventPage {
            object: "cma.agent.event_page",
            schema_version: 1,
            agent_id: expected["agent_id"].as_str().unwrap().into(),
            incarnation: expected["incarnation"].as_str().unwrap().into(),
            through_cursor: expected["through_cursor"].as_str().unwrap().into(),
            items: expected["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(item)
                .collect(),
            next_page_cursor: None,
        };
        assert_eq!(serde_json::to_value(page).unwrap(), *expected);
    }
    for scenario in ["live_requested", "live_resolved", "live_terminal"] {
        let expected = &corpus[scenario]["data"];
        let event = DurableEvent {
            agent_id: expected["agent_id"].as_str().unwrap().into(),
            incarnation: expected["incarnation"].as_str().unwrap().into(),
            item: item(expected),
        };
        assert_eq!(serde_json::to_value(event).unwrap(), *expected);
    }
}

#[tokio::test]
async fn head_negotiates_current_viewer_policy_without_changing_its_metadata_shape() {
    for buffered in [false, true] {
        let f = fixture(true, false).await;
        crate::agent_v2::head_tests::share(&f).await;
        let path = format!("{EVENTS}/head");
        for (subject, origin, requested, action) in [
            ("owner-a", "https://identity.example.test", true, "review"),
            ("reader", "https://identity.example.test", true, "none"),
            ("owner-a", "", true, "none"),
            ("owner-a", "https://identity.example.test", false, "none"),
        ] {
            let router = router(
                f.state
                    .clone()
                    .with_single_fence_pages(buffered)
                    .with_external_approval_review_origin(origin),
            );
            let mut request = request(Method::GET, &path, subject, None, None);
            if requested {
                request
                    .headers_mut()
                    .insert("prefer", PREFERENCE.parse().unwrap());
            }
            let response = router.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            if requested {
                assert_eq!(response.headers()["preference-applied"], PREFERENCE);
                assert_eq!(response.headers()["cma-external-approval-action"], action);
                assert_eq!(
                    response
                        .headers()
                        .get("cma-approval-review-origin")
                        .map(|value| value.to_str().unwrap()),
                    (!origin.is_empty()).then_some(origin)
                );
            } else {
                for name in [
                    "preference-applied",
                    "cma-external-approval-action",
                    "cma-approval-review-origin",
                ] {
                    assert!(!response.headers().contains_key(name));
                }
            }
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["object"], "cma.agent.event_head");
            assert_eq!(body["schema_version"], 1);
            assert_eq!(body["active_turn"]["kind"]["type"], "turn_started");
            assert!(!body.to_string().contains("approve_url"));
        }
    }
}

#[tokio::test]
async fn external_wait_snapshot_and_sse_require_acknowledged_capability() {
    for buffered in [false, true] {
        let f = fixture(false, false).await;
        let state = f
            .state
            .clone()
            .with_single_fence_pages(buffered)
            .with_external_approval_review_origin("https://identity.example.test");
        let router = router(state);
        append(&f.store, pending()).await;
        assert!(f
            .store
            .pending_agent_approvals(&layer2_core::AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .is_empty());
        for path in [SNAPSHOT.to_owned(), format!("{EVENTS}?follow=false")] {
            let refused = router
                .clone()
                .oneshot(request(Method::GET, &path, "owner-a", None, None))
                .await
                .unwrap();
            let expected: Value =
                serde_json::from_str(include_str!("capability_fixtures.json")).unwrap();
            let expected = &expected["http_refusal"];
            let status = u64::from(refused.status().as_u16());
            let content_type = refused.headers()["content-type"]
                .to_str()
                .unwrap()
                .to_owned();
            assert_eq!(status, expected["status"].as_u64().unwrap());
            assert_eq!(content_type, expected["content_type"].as_str().unwrap());
            for header in [
                "preference-applied",
                "cma-approval-review-origin",
                "cma-external-approval-action",
            ] {
                assert!(!refused.headers().contains_key(header));
            }
            let body = to_bytes(refused.into_body(), 1024 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body, expected["body"]);
            println!(
                "EXTERNAL_APPROVAL_HTTP_REFUSAL {}",
                serde_json::json!({
                    "buffered": buffered,
                    "snapshot": path == SNAPSHOT,
                    "status": status,
                    "content_type": content_type,
                    "body": body
                })
            );
            let mut qualified = request(Method::GET, &path, "owner-a", None, None);
            qualified
                .headers_mut()
                .insert("prefer", PREFERENCE.parse().unwrap());
            let response = router.clone().oneshot(qualified).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["preference-applied"], PREFERENCE);
            assert_eq!(
                response.headers()["cma-approval-review-origin"],
                "https://identity.example.test"
            );
            assert_eq!(response.headers()["cma-external-approval-action"], "review");
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let body = String::from_utf8(body.to_vec()).unwrap();
            assert!(body.contains("https://identity.example.test/assistant/approvals"));
            assert!(body.contains("external_approval_requested"));
            assert!(!body.contains("runtime_locator"));
            if path == SNAPSHOT {
                let page: Value = serde_json::from_str(&body).unwrap();
                let row = page["items"].as_array().unwrap().last().unwrap();
                assert_eq!(page["schema_version"], 1);
                assert!(row["response_binding"].is_null());
                assert_eq!(row["kind"]["waiting_since"], "2026-01-01T00:00:01.000Z");
                assert_eq!(row["kind"]["deadline_at"], "2026-01-01T00:05:01.000Z");
            }
        }
        append(
            &f.store,
            AgentLogKind::ExternalApprovalResolved {
                approval_id: "ea_11111111111111111111111111111111".into(),
                provider: "nyxid".into(),
                turn_sequence: 3,
                revision: 2,
                resolution: ExternalApprovalResolution::Approved,
            },
        )
        .await;
        let mut qualified = request(Method::GET, SNAPSHOT, "owner-a", None, None);
        qualified
            .headers_mut()
            .insert("prefer", PREFERENCE.parse().unwrap());
        let response = router.oneshot(qualified).await.unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(
            body["items"].as_array().unwrap().last().unwrap()["kind"]["resolution"],
            "approved"
        );
    }
}

#[tokio::test]
async fn an_open_unnegotiated_stream_refuses_the_first_external_row_without_issuing_its_cursor() {
    let f = fixture(false, false).await;
    let response = f
        .router
        .clone()
        .oneshot(request(Method::GET, EVENTS, "owner-a", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("preference-applied"));
    append(&f.store, pending()).await;
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    let expected: Value = serde_json::from_str(include_str!("capability_fixtures.json")).unwrap();
    let frames: Vec<_> = text
        .split("\n\n")
        .filter(|frame| !frame.is_empty())
        .collect();
    let error: Vec<_> = frames.last().unwrap().lines().collect();
    assert_eq!(
        error.len(),
        2,
        "error must contain only event and data, without id"
    );
    assert_eq!(
        error[0],
        format!(
            "event: {}",
            expected["stream_error"]["event"].as_str().unwrap()
        )
    );
    assert_eq!(
        serde_json::from_str::<Value>(error[1].strip_prefix("data: ").unwrap()).unwrap(),
        expected["stream_error"]["data"]
    );
    assert_eq!(text.matches("event: cma.agent.error").count(), 1);
    assert!(!text.contains("external_approval_requested"));
    let ids: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.last().unwrap().ends_with(".2"));
    assert!(!text
        .split("event: cma.agent.error")
        .last()
        .unwrap()
        .contains("id:"));
    println!(
        "EXTERNAL_APPROVAL_STREAM_REFUSAL {}",
        serde_json::json!({
            "body": text,
            "accepted_cursors": ids
        })
    );
}

#[tokio::test]
async fn a_native_approval_keeps_its_existing_unnegotiated_contract() {
    let f = fixture(false, false).await;
    append(
        &f.store,
        AgentLogKind::ApprovalRequested {
            approval_id: "native-request".into(),
            action: Some("Synthetic action".into()),
            detail: None,
            runtime_locator: Some("private-native-locator".into()),
        },
    )
    .await;
    let response = f
        .router
        .oneshot(request(Method::GET, SNAPSHOT, "owner-a", None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("preference-applied"));
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let kind = &body["items"].as_array().unwrap().last().unwrap()["kind"];
    assert_eq!(kind["type"], "approval_requested");
    assert!(kind.get("approve_url").is_none());
    assert!(kind.get("runtime_locator").is_none());
}

#[tokio::test]
async fn shared_history_explains_waiting_without_an_actionable_link() {
    let f = fixture(false, false).await;
    crate::agent_v2::head_tests::share(&f).await;
    append(&f.store, pending()).await;
    let router = router(
        f.state
            .with_external_approval_review_origin("https://identity.example.test"),
    );
    let mut qualified = request(Method::GET, SNAPSHOT, "reader", None, None);
    qualified
        .headers_mut()
        .insert("prefer", PREFERENCE.parse().unwrap());
    let response = router.oneshot(qualified).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cma-external-approval-action"], "none");
    let page: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let kind = &page["items"].as_array().unwrap().last().unwrap()["kind"];
    assert_eq!(kind["type"], "external_approval_requested");
    assert!(kind["approve_url"].is_null());
}

#[test]
fn restricted_or_invalid_policy_removes_provider_supplied_actions() {
    let mut row = super::super::shapes::EventItem {
        retained_output: None, cursor: "cursor".into(), sequence: 9,
        appended_at: "2026-01-01T00:00:05.000Z".into(),
        kind: Some(serde_json::from_value(json!({
            "type":"external_approval_requested", "approval_id":"ea_11111111111111111111111111111111",
            "provider":"nyxid", "turn_sequence":3, "revision":1,
            "waiting_since":"2026-01-01T00:00:01.000Z", "deadline_at":"2026-01-01T00:05:01.000Z",
            "approve_url":"https://untrusted.example.test/approve"
        })).unwrap()), response_binding: None, message_command_id: None,
    };
    for origin in [
        ReviewOrigin::default(),
        ReviewOrigin::configured("https://identity.example.test"),
    ] {
        let preference = Preference {
            requested: true,
            origin,
            review: false,
        };
        preference.project(std::slice::from_mut(&mut row)).unwrap();
        assert!(serde_json::to_value(&row).unwrap()["kind"]["approve_url"].is_null());
        let mut headers = HeaderMap::new();
        preference.headers(&mut headers);
        assert_eq!(headers["cma-external-approval-action"], "none");
    }
}
