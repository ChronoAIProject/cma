//! Opt-in multiplexing through real ingress, provider runners and delivery.
use super::{app::App, fake_provider::FakeProviderConfig, policy::REVIEWER, requests::event};
use serde_json::{json, Value};
use std::time::Duration;

type Frames = Vec<(String, Value)>;
async fn frames(response: reqwest::Response) -> Frames {
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = tokio::time::timeout(Duration::from_secs(5), response.text())
        .await
        .expect("multiplexed stream ends")
        .unwrap();
    parse(&body)
}
fn parse(body: &str) -> Frames {
    body.split("\n\n")
        .filter(|frame| frame.starts_with("event:"))
        .map(|frame| {
            let field = |prefix| {
                frame
                    .lines()
                    .find_map(|line| line.strip_prefix(prefix))
                    .unwrap()
            };
            (
                field("event: ").to_owned(),
                serde_json::from_str(field("data: ")).unwrap(),
            )
        })
        .collect()
}
fn final_frame(frames: &Frames, ids: &[&str]) {
    assert_eq!(
        frames
            .iter()
            .filter(|(name, _)| name == "cmaeg.fanout.completed")
            .count(),
        1
    );
    let (name, data) = frames.last().unwrap();
    assert_eq!(name, "cmaeg.fanout.completed");
    assert_eq!(
        data["consumers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|consumer| consumer["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
}

#[tokio::test]
async fn stream_all_multiplexes_both_consumers_in_pinned_order() {
    let mut app = App::new(true, |raw| {
        raw["reply"]["sinks"] = json!([{"type":"sync"}]);
    })
    .await;
    let response = app
        .event_mode(
            &app.channel,
            event("all", Some("reviewer,main")),
            Some("stream_all"),
        )
        .await;
    let frames = frames(response).await;
    app.drain().await;
    final_frame(&frames, &[REVIEWER, "default"]);
    assert_eq!(frames.len(), 5);
    for id in [REVIEWER, "default"] {
        let child: Vec<_> = frames
            .iter()
            .filter(|(_, data)| data["consumer"]["id"] == id)
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(child, ["cmaeg.consumer.delta", "cmaeg.consumer.completed"]);
    }
    assert_eq!([app.turns(0).len(), app.turns(1).len()], [1, 1]);
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn stream_all_single_routed_and_unrouted_events_share_the_same_frame_protocol() {
    for routed in [false, true] {
        let mut app = App::new(true, |raw| {
            if !routed {
                raw.as_object_mut().unwrap().remove("routing");
                raw["secrets"]
                    .as_object_mut()
                    .unwrap()
                    .remove("consumer_keys");
            }
        })
        .await;
        let target = routed.then_some("reviewer");
        let response = app
            .event_mode(&app.channel, event("single", target), Some("stream_all"))
            .await;
        let frames = frames(response).await;
        app.drain().await;
        let id = if routed { REVIEWER } else { "default" };
        final_frame(&frames, &[id]);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].0, "cmaeg.consumer.delta");
        assert_eq!(frames[1].0, "cmaeg.consumer.completed");
        for (_, data) in &frames[..2] {
            assert_eq!(data["consumer"]["id"], id);
            assert_eq!(
                data["consumer"]["name"],
                if routed { "reviewer" } else { "default" }
            );
        }
        assert!(app.waiters.is_empty());
    }
}

#[tokio::test]
async fn disabled_or_missing_sync_uses_the_existing_invalid_mode_bytes_before_routing() {
    for enabled in [false, true] {
        let mut app = App::new(enabled, |raw| {
            if enabled {
                raw["reply"]["sinks"] = json!([]);
            }
        })
        .await;
        let baseline = app
            .event_mode(&app.channel, event("baseline", None), Some("unknown"))
            .await;
        assert_eq!(baseline.status(), 400);
        let expected = baseline.bytes().await.unwrap();
        let response = app
            .event_mode(
                &app.channel,
                event("invalid", Some("ghost")),
                Some("stream_all"),
            )
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(response.bytes().await.unwrap(), expected);
        assert!(app.waiters.is_empty());
        app.drain().await;
        assert!(app.turns(0).is_empty() && app.turns(1).is_empty());
    }
}

#[tokio::test]
async fn stream_all_routing_refusal_never_registers_or_launches() {
    let mut app = App::new(true, |_| {}).await;
    let response = app
        .event_mode(
            &app.channel,
            event("missing", Some("ghost")),
            Some("stream_all"),
        )
        .await;
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["detail"],
        "target_unknown:ghost"
    );
    assert!(app.waiters.is_empty());
    app.drain().await;
    assert!(app.turns(0).is_empty() && app.turns(1).is_empty());
}

#[tokio::test]
async fn rejected_or_unaccepted_eof_child_has_one_terminal_and_one_failed_webhook() {
    for eof in [false, true] {
        let main = FakeProviderConfig {
            terminal_delay: Duration::from_millis(250),
            ..Default::default()
        };
        let reviewer = if eof {
            FakeProviderConfig {
                frames: Some(vec![]),
                ..Default::default()
            }
        } else {
            FakeProviderConfig {
                reject_response: Some((422, "profile_refused".into())),
                ..Default::default()
            }
        };
        let mut app = App::configured(true, true, [main, reviewer], |_| {}).await;
        let response = app
            .event_mode(
                &app.channel,
                event("failure", Some("main,reviewer")),
                Some("stream_all"),
            )
            .await;
        let frames = frames(response).await;
        app.drain().await;
        final_frame(&frames, &["default", REVIEWER]);
        let terminals: Vec<_> = frames
            .iter()
            .filter(|(name, _)| {
                name == "cmaeg.consumer.failed" || name == "cmaeg.consumer.completed"
            })
            .collect();
        assert_eq!(terminals.len(), 2);
        assert_eq!(terminals[0].0, "cmaeg.consumer.failed");
        assert_eq!(terminals[0].1["consumer"]["id"], REVIEWER);
        assert_eq!(terminals[1].0, "cmaeg.consumer.completed");
        assert_eq!(terminals[1].1["consumer"]["id"], "default");
        let calls = app.peers.calls.0.lock().unwrap();
        let failed: Vec<_> = calls
            .iter()
            .filter(|(kind, body)| kind == "webhook" && body["type"] == "cmaeg.response.failed")
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].1["data"]["consumer"]["id"], REVIEWER);
        assert!(app.waiters.is_empty());
    }
}

#[tokio::test]
async fn refused_primary_keeps_an_admitted_sibling_stream_open() {
    let rejected = FakeProviderConfig {
        reject_response: Some((422, "profile_refused".into())),
        ..Default::default()
    };
    let accepted = FakeProviderConfig {
        terminal_delay: Duration::from_millis(150),
        ..Default::default()
    };
    let mut app = App::configured(true, true, [rejected, accepted], |_| {}).await;
    let response = app
        .event_mode(
            &app.channel,
            event("primary-failed", Some("main,reviewer")),
            Some("stream_all"),
        )
        .await;
    let frames = frames(response).await;
    app.drain().await;
    final_frame(&frames, &["default", REVIEWER]);
    assert!(frames.iter().any(
        |(name, data)| name == "cmaeg.consumer.completed" && data["consumer"]["id"] == REVIEWER
    ));
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn all_refused_drops_the_multiplex_waiter_and_preserves_the_problem_response() {
    let rejected = FakeProviderConfig {
        reject_response: Some((422, "profile_refused".into())),
        ..Default::default()
    };
    let mut app = App::configured(true, true, [rejected.clone(), rejected], |_| {}).await;
    let baseline = app
        .event_mode(
            &app.channel,
            event("plain-refused", Some("main")),
            Some("stream"),
        )
        .await;
    let status = baseline.status();
    assert!(status.is_client_error() || status.is_server_error());
    let expected = baseline.bytes().await.unwrap();
    let response = app
        .event_mode(
            &app.channel,
            event("all-refused", Some("main,reviewer")),
            Some("stream_all"),
        )
        .await;
    assert_eq!(response.status(), status);
    assert_eq!(response.bytes().await.unwrap(), expected);
    assert!(app.waiters.is_empty());
    app.drain().await;
}
