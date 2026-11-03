#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{
    Error,
    agent_events_v2::{
        AgentCursor, AgentEventKind, AgentFrame, AgentPageCursor, SnapshotOptions, TailOptions,
    },
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wire::{Expected, WireServer};

const AGENT: &str = "agt_fixture";
const SNAPSHOT: &str = "/api/v2/agents/agt_fixture/events/snapshot";
const EVENTS: &str = "/api/v2/agents/agt_fixture/events";
fn fixture() -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/agent-events-v2.json"
    ))
    .unwrap();
    corpus["cases"][0]["response"]["body"].clone()
}
fn cursor(sequence: u64) -> String {
    format!("ag1.{}.{}", "a".repeat(64), sequence)
}
fn event(sequence: u64) -> Value {
    json!({"agent_id":AGENT,"incarnation":"a".repeat(64),"cursor":cursor(sequence),"sequence":sequence,"appended_at":"2026-09-16T00:00:00Z","kind":null,"response_binding":null})
}
fn frame(name: &str, data: &Value, id: Option<&str>) -> String {
    format!(
        "{}event: {name}\ndata: {data}\n\n",
        id.map_or(String::new(), |id| format!("id: {id}\n"))
    )
}
fn expected(path: &str, value: Value) -> Expected {
    Expected {
        method: "GET".into(),
        path: path.into(),
        headers: wire::headers(true, "application/json"),
        status: 200,
        response_headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        response: serde_json::to_vec(&value).unwrap(),
        ..Default::default()
    }
}
fn sse(path: &str, body: String) -> Expected {
    Expected {
        method: "GET".into(),
        path: path.into(),
        headers: wire::headers(true, "text/event-stream"),
        status: 200,
        response_headers: BTreeMap::from([("content-type".into(), "text/event-stream".into())]),
        chunks: Some(body.as_bytes().chunks(65_536).map(<[u8]>::to_vec).collect()),
        ..Default::default()
    }
}

#[tokio::test]
async fn live_observation_timestamps_are_opt_in_and_allow_older_servers() {
    for observed in [None, Some("2026-10-20T16:31:00Z")] {
        let server = WireServer::new().await;
        let mut live = json!({"agent_id":AGENT,"incarnation":"a".repeat(64),"turn_sequence":1,"after_sequence":2,"message_id":"msg","kind":"text_delta","delta":"reply"});
        if let Some(at) = observed {
            live["observed_at"] = json!(at);
        }
        let mut expected = sse(
            &format!("{EVENTS}?after_cursor={}", cursor(2)),
            frame("cma.agent.live", &live, None),
        );
        expected
            .headers
            .insert("prefer".into(), "cma-live-observed-at".into());
        server.expect(expected);
        let mut stream = server
            .client
            .agent_events_v2()
            .events(
                AGENT,
                &TailOptions {
                    after_cursor: Some(AgentCursor::parse(cursor(2)).unwrap()),
                    live_timestamps: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        stream
            .consume_next(|frame| async move {
                match frame {
                    AgentFrame::Live(live) => assert_eq!(live.observed_at.as_deref(), observed),
                    _ => panic!("expected live fragment"),
                }
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
        server.assert_drained();
    }
}

#[tokio::test]
async fn retains_rich_recorded_snapshot_and_exact_turn_binding() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    let value = fixture();
    server.expect(expected(SNAPSHOT, value.clone()));
    let page = server
        .client
        .agent_events_v2()
        .snapshot(AGENT, &SnapshotOptions::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 14);
    assert_eq!(
        page.through_cursor.as_str(),
        value["through_cursor"].as_str().unwrap()
    );
    assert!(matches!(
        page.items[3].kind,
        Some(AgentEventKind::TurnStarted { turn_sequence: 4 })
    ));
    assert_eq!(
        page.items[3].response_binding.as_ref().unwrap().response_id,
        "resp_control_source"
    );
    server.assert_drained();
}
#[test]
fn opaque_cursor_families_and_canonical_u64_bounds_are_distinct() {
    for value in [
        "1".into(),
        "ag1.bad.1".into(),
        format!("ag1.{}.01", "a".repeat(64)),
        format!("ag1.{}.18446744073709551616", "a".repeat(64)),
        format!("ags1.{}.2.1", "a".repeat(64)),
    ] {
        assert!(AgentCursor::parse(value).is_err());
    }
    assert!(AgentCursor::parse(cursor(u64::MAX)).is_ok());
    for value in [
        cursor(1),
        format!("ags1.{}.1.1", "a".repeat(64)),
        format!("ags1.{}.2.3", "a".repeat(64)),
    ] {
        assert!(AgentPageCursor::parse(value).is_err());
    }
}
#[tokio::test]
async fn malformed_snapshot_never_reaches_the_consumer() {
    let original = fixture();
    let mut cases = Vec::new();
    for (field, value) in [
        ("schema_version", json!(2)),
        ("agent_id", json!("other")),
        ("incarnation", json!("b".repeat(64))),
        ("runtime_locator", json!("private")),
    ] {
        let mut v = original.clone();
        v[field] = value;
        cases.push(v);
    }
    for field in ["kind", "response_binding"] {
        let mut v = original.clone();
        v["items"][0].as_object_mut().unwrap().remove(field);
        cases.push(v);
    }
    for (field, value) in [
        ("sequence", json!(0)),
        ("cursor", json!(cursor(1))),
        (
            "kind",
            json!({"type":"message","role":"assistant","content":"safe","reasoning":"private"}),
        ),
        (
            "kind",
            json!({"type":"tool_updated","call_id":"call","status":"completed"}),
        ),
        (
            "response_binding",
            json!({"command_id":"cmd","response_id":"resp","turn_sequence":1}),
        ),
    ] {
        let mut v = original.clone();
        v["items"][0][field] = value;
        cases.push(v);
    }
    let mut v = original.clone();
    v["items"].as_array_mut().unwrap().remove(4);
    cases.push(v);
    let mut v = original.clone();
    v["next_page_cursor"] = json!(format!("ags1.{}.15.14", v["incarnation"].as_str().unwrap()));
    cases.push(v);
    for value in cases {
        let server = WireServer::new().await;
        server.expect(expected(SNAPSHOT, value));
        assert!(matches!(
            server
                .client
                .agent_events_v2()
                .snapshot(AGENT, &SnapshotOptions::default())
                .await,
            Err(Error::Protocol { .. })
        ));
        server.assert_drained();
    }
}
#[tokio::test]
async fn consumer_failure_preserves_prior_ack_and_reconnect_is_explicit_read_only() {
    let server = WireServer::new().await;
    let after = AgentCursor::parse(cursor(1)).unwrap();
    let path = format!("{EVENTS}?after_cursor={}", after.as_str());
    server.expect(sse(
        &path,
        frame("cma.agent.event", &event(2), Some(&cursor(2)))
            + &frame("cma.agent.event", &event(3), Some(&cursor(3))),
    ));
    let mut stream = server
        .client
        .agent_events_v2()
        .events(
            AGENT,
            &TailOptions {
                after_cursor: Some(after),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    stream
        .consume_next(|frame| async move {
            assert!(matches!(frame, AgentFrame::Event(_)));
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stream.acknowledged_cursor().unwrap().as_str(), cursor(2));
    assert!(
        stream
            .consume_next(|_| async {
                Err(Error::Protocol {
                    message: "consumer failed".into(),
                })
            })
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(stream.acknowledged_cursor().unwrap().as_str(), cursor(2));
    assert!(stream.consume_next(|_| async { Ok(()) }).await.is_none());
    assert_eq!(server.request_count(), 1);
    let retained = stream.acknowledged_cursor().cloned();
    server.expect(sse(
        &format!("{EVENTS}?after_cursor={}&follow=false", cursor(2)),
        frame("cma.agent.event", &event(3), Some(&cursor(3))),
    ));
    let mut resumed = server
        .client
        .agent_events_v2()
        .events(
            AGENT,
            &TailOptions {
                after_cursor: retained,
                follow: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    resumed
        .consume_next(|_| async { Ok(()) })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.acknowledged_cursor().unwrap().as_str(), cursor(3));
    server.assert_drained();
}
#[tokio::test]
async fn ephemeral_fragments_never_advance_the_durable_cursor() {
    let server = WireServer::new().await;
    let after = AgentCursor::parse(cursor(2)).unwrap();
    let live = json!({"agent_id":AGENT,"incarnation":"a".repeat(64),"turn_sequence":1,"after_sequence":2,"message_id":"msg","kind":"reasoning_summary_delta","delta":"ephemeral only"});
    let gap = json!({"agent_id":AGENT,"incarnation":"a".repeat(64),"turn_sequence":1,"frames":null,"reason":"reconnected"});
    server.expect(sse(
        &format!("{EVENTS}?after_cursor={}", after.as_str()),
        frame("cma.agent.live", &live, None) + &frame("cma.agent.live_gap", &gap, None),
    ));
    let mut stream = server
        .client
        .agent_events_v2()
        .events(
            AGENT,
            &TailOptions {
                after_cursor: Some(after.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for _ in 0..2 {
        stream
            .consume_next(|_| async { Ok(()) })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stream.acknowledged_cursor(), Some(&after));
    }
    assert!(stream.consume_next(|_| async { Ok(()) }).await.is_none());
    server.assert_drained();
}
#[tokio::test]
async fn malformed_sse_identity_ids_and_incomplete_frames_do_not_acknowledge() {
    let bad_live = json!({"agent_id":AGENT,"incarnation":"a".repeat(64),"turn_sequence":1,"after_sequence":1,"message_id":"msg","kind":"text_delta","delta":"x"});
    let gap = json!({"agent_id":AGENT,"incarnation":"a".repeat(64),"turn_sequence":null,"frames":1,"reason":"reconnected"});
    let mut wrong_agent = event(1);
    wrong_agent["agent_id"] = json!("other");
    let bodies = [
        frame("cma.agent.event", &event(1), None),
        frame("cma.agent.event", &event(2), Some(&cursor(2))),
        frame("cma.agent.event", &wrong_agent, Some(&cursor(1))),
        frame("cma.agent.live", &bad_live, Some(&cursor(1))),
        frame("cma.agent.live_gap", &gap, None),
        "event: cma.agent.event\ndata: {\n\n".into(),
        frame("cma.agent.event", &event(1), Some(&cursor(1)))
            .trim_end()
            .to_owned(),
    ];
    for body in bodies {
        let server = WireServer::new().await;
        server.expect(sse(EVENTS, body));
        let mut stream = server
            .client
            .agent_events_v2()
            .events(AGENT, &TailOptions::default())
            .await
            .unwrap();
        let result = stream
            .consume_next(|_| async { panic!("malformed frame reached consumer") })
            .await;
        assert!(matches!(result, Some(Err(Error::Protocol { .. }))));
        assert!(stream.acknowledged_cursor().is_none());
        server.assert_drained();
    }
}
#[tokio::test]
async fn stream_allows_large_valid_agent_frames_but_rejects_oversized_and_partial_bytes() {
    for (size, valid) in [(2 * 1024 * 1024, true), (4 * 1024 * 1024, false)] {
        let server = WireServer::new().await;
        let mut data = event(1);
        data["kind"] = json!({"type":"message","role":"assistant","content":"x".repeat(size)});
        server.expect(sse(
            EVENTS,
            frame("cma.agent.event", &data, Some(&cursor(1))),
        ));
        let mut stream = server
            .client
            .agent_events_v2()
            .events(AGENT, &TailOptions::default())
            .await
            .unwrap();
        assert_eq!(
            stream
                .consume_next(|_| async { Ok(()) })
                .await
                .unwrap()
                .is_ok(),
            valid
        );
        assert_eq!(stream.acknowledged_cursor().is_some(), valid);
        server.assert_drained();
    }
    let server = WireServer::new().await;
    server.expect(sse(EVENTS, "data: ".to_owned() + &"🙂".repeat(1_049_000)));
    let mut stream = server
        .client
        .agent_events_v2()
        .events(AGENT, &TailOptions::default())
        .await
        .unwrap();
    assert!(
        stream
            .consume_next(|_| async { Ok(()) })
            .await
            .unwrap()
            .is_err()
    );
    assert!(stream.acknowledged_cursor().is_none());
}
#[tokio::test]
async fn page_bytes_and_query_header_disagreement_fail_without_mutation() {
    let server = WireServer::new().await;
    let mut response = expected(SNAPSHOT, fixture());
    response.response = [vec![b' '; 4 * 1024 * 1024], response.response].concat();
    server.expect(response);
    assert!(matches!(
        server
            .client
            .agent_events_v2()
            .snapshot(AGENT, &SnapshotOptions::default())
            .await,
        Err(Error::Protocol { .. })
    ));
    assert!(matches!(
        server
            .client
            .agent_events_v2()
            .events(
                AGENT,
                &TailOptions {
                    after_cursor: Some(AgentCursor::parse(cursor(1)).unwrap()),
                    last_event_id: Some(AgentCursor::parse(cursor(2)).unwrap()),
                    follow: None,
                    live_timestamps: false,
                    tool_output: false,
                }
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}
