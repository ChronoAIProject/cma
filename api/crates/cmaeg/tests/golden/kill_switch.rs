//! I13 at real HTTP and provider/delivery boundaries. Card-consumption cases
//! live in routing/cards.rs; legacy I5/I6/I7 fixtures remain unchanged.
use super::{app::App, fixture::assert_golden, policy::REVIEWER, requests::event};
use serde_json::{json, Value};

async fn response(response: reqwest::Response) -> Value {
    let status = response.status().as_u16();
    let retry = response
        .headers()
        .get("retry-after")
        .map(|v| v.to_str().unwrap().to_owned());
    let mut body: Value = response.json().await.unwrap();
    if body.get("conversation_id").is_some() {
        body["conversation_id"] = "<conversation>".into();
    }
    json!({"status":status,"retry_after":retry,"body":body})
}

#[tokio::test]
async fn i13_explicit_targets_fail_closed_and_default_paths_preserve_raw_input() {
    let mut app = App::new(false, |raw| {
        raw["reply"]["auto_final"] = false.into();
        raw["routing"]["rules"] = json!([{"id":"all","when":{},"targets":["default",REVIEWER]}]);
    })
    .await;
    let mut rows = Vec::new();
    for (i, text) in ["@reviewer x", "/to reviewer x", "/to ghost x"]
        .into_iter()
        .enumerate()
    {
        let relay = response(app.callback(text, &format!("refused-{i}")).await).await;
        let native = response(app.native(text, i as u64 + 100).await).await;
        for result in [&relay, &native] {
            assert_eq!(result["status"], 202);
            assert_eq!(
                result["body"],
                json!({"status":"refused","reason":"consumer_unavailable"})
            );
        }
        rows.push(json!({"text":text,"relay":relay,"native":native}));
    }
    for targets in ["reviewer", "ghost"] {
        for stream in [false, true] {
            let result = response(
                app.event(&app.channel, event("custom-refused", Some(targets)), stream)
                    .await,
            )
            .await;
            assert_eq!(result["status"], 503);
            assert_eq!(result["retry_after"], "300");
            assert!(app.waiters.is_empty());
            rows.push(json!({"targets":targets,"stream":stream,"response":result}));
        }
    }
    app.quiet().await;
    assert!(app.turns(0).is_empty() && app.turns(1).is_empty());
    for (i, text) in ["@main x", "/to main x", "ordinary x"]
        .into_iter()
        .enumerate()
    {
        rows.push(json!({"text":text,"relay":response(app.callback(text, &format!("legacy-{i}")).await).await,
            "native":response(app.native(text, i as u64 + 200).await).await}));
        app.quiet().await;
        assert_eq!(app.turns(0).len(), (i + 1) * 2);
        for turn in app.turns(0).iter().rev().take(2) {
            assert!(turn["event_context"]["activity"].get("routing").is_none());
            assert!(turn["metadata"].get("cmaeg.consumer").is_none());
            assert_eq!(turn["input"][0]["content"][0]["text"], text);
        }
    }
    for (i, targets) in [Some("main"), None].into_iter().enumerate() {
        rows.push(json!({"targets":targets,"custom":response(app.event(&app.channel, event("custom-legacy", targets), false).await).await}));
        app.quiet().await;
        let turns = app.turns(0);
        assert_eq!(turns.len(), 7 + i);
        let turn = turns.last().unwrap();
        assert!(turn["event_context"]["activity"].get("routing").is_none());
        assert!(turn["metadata"].get("cmaeg.consumer").is_none());
        assert_eq!(
            turn["event_context"]["activity"]["kind"]["ce"]["cmaegtargets"].as_str(),
            targets
        );
    }
    assert!(app.turns(1).is_empty());
    let input = app.turns(0).last().unwrap()["input"].clone();
    let mut raw = app.channel.definition.to_blob_json();
    raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
    raw.as_object_mut().unwrap().remove("routing");
    raw["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    let legacy = app.seed(raw).await;
    assert_eq!(
        app.event(&legacy, event("custom-legacy", None), false)
            .await
            .status(),
        202
    );
    app.quiet().await;
    assert_eq!(app.turns(0).len(), 9);
    assert_eq!(app.turns(0).last().unwrap()["input"], input);
    app.drain().await;
    assert!(app.turns(1).is_empty());
    assert!(
        app.peers.chat_calls().is_empty(),
        "kill switch must emit no notices"
    );
    let actual = serde_json::to_string_pretty(&rows).unwrap() + "\n";
    assert_golden("kill-switch/ingress.json", &actual);
}

#[tokio::test]
async fn i13_rules_excluding_default_close_only_matching_events() {
    let mut app = App::new(false, |raw| {
        raw["reply"]["auto_final"] = false.into();
        raw["routing"]["rules"] = json!([{"id":"native","when":{"sources":["telegram"]},"targets":[REVIEWER]},
            {"id":"blocked","when":{"event_types":["blocked"]},"targets":[REVIEWER]}]);
    }).await;
    let result = response(app.native("ordinary", 301).await).await;
    assert_eq!(
        result["body"],
        json!({"status":"refused","reason":"consumer_unavailable"})
    );
    let mut blocked = event("blocked-custom", None);
    blocked["type"] = "blocked".into();
    let result = response(app.event(&app.channel, blocked, false).await).await;
    assert_eq!(result["status"], 503);
    assert_eq!(result["retry_after"], "300");
    assert!(app.turns(0).is_empty() && app.turns(1).is_empty());
    assert_eq!(app.native("@main ordinary", 302).await.status(), 202);
    assert_eq!(
        app.callback("ordinary", "ordinary-relay").await.status(),
        202
    );
    assert_eq!(
        app.event(&app.channel, event("ordinary-custom", None), false)
            .await
            .status(),
        202
    );
    app.drain().await;
    assert_eq!(app.turns(0).len(), 3);
    assert!(app.turns(1).is_empty());
    assert!(app.peers.chat_calls().is_empty());
}

#[tokio::test]
async fn i13_admitted_turn_finishes_but_off_config_refuses_its_default_reply() {
    use super::{fake_provider::FakeProviderConfig, logs::Logs, policy};
    use cmaeg::{
        auth::AgentCaller,
        delivery::intents::{DeliveryCommand, DeliveryOutcome, ReplySource},
    };
    use cmaeg_core::{channel::ReplyChoice, seal::EventRef};
    use std::time::Duration;
    let logs = Logs::default();
    let _capture = logs.capture();
    let mut app = App::configured(
        true,
        false,
        [
            Default::default(),
            FakeProviderConfig {
                terminal_delay: Duration::from_secs(2),
                ..Default::default()
            },
        ],
        |raw| {
            raw["routing"]["default_reply"] =
                json!({"type":"telegram_chat","source_id":"telegram","chat_id":"-1001234567890"});
            raw["routing"]["consumers"][0]["reply"] = json!({"destination":"default"});
        },
    )
    .await;
    // Separate immutable snapshots model a turn admitted before a flag-off
    // rollout; production has no runtime flag reload operation.
    assert!(app.state.routing.multi_consumer_enabled);
    assert!(!app.tools.multi_consumer_enabled);
    assert_eq!(
        app.event(
            &app.channel,
            event("already-admitted", Some("reviewer")),
            false
        )
        .await
        .status(),
        202
    );
    assert!(!app.tracker.is_empty());
    let turns = app.turns(1);
    let reference = turns[0]["event_context"]["event_ref"].as_str().unwrap();
    let response = app
        .client
        .post(format!(
            "{}/v1/events/{reference}/replies?destination=default",
            app.base
        ))
        .bearer_auth(policy::agent_key(REVIEWER))
        .json(&json!({"text":"must not reach default"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "invalid_request");
    assert_eq!(problem["detail"], "multi_consumer_disabled");

    // Exercise an already-authorized command at the queue boundary separately
    // from the public handler, which correctly refuses new default requests.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", policy::agent_key(REVIEWER))
            .parse()
            .unwrap(),
    );
    let (channel, event, consumer) = app
        .tools
        .authorize_event(
            AgentCaller::from_headers(&headers).unwrap(),
            &EventRef::from_sealed(reference.to_owned()),
            "replyToEvent",
        )
        .await
        .unwrap();
    assert_eq!(consumer.as_str(), REVIEWER);
    assert_eq!(event.consumer.as_ref(), Some(&consumer));
    assert_eq!(event.event_id.as_str(), "already-admitted");
    let (receipt, received) = tokio::sync::oneshot::channel();
    app.tools
        .delivery
        .submit(DeliveryCommand::Reply {
            trace: tracing::Span::current(),
            channel,
            event,
            message: serde_json::from_value(json!({"text":"already-authorized output"})).unwrap(),
            source: ReplySource::Tool,
            destination: Some(ReplyChoice::Default),
            receipt,
        })
        .await
        .unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(3), received)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Rejected {
            reason: "default_reply_unavailable".into()
        }
    );
    app.drain().await;
    assert_eq!(app.turns(1).len(), 1);
    assert!(app.peers.chat_calls().is_empty());
    let logs = logs.text();
    let channel_hash = cmaeg::telemetry::channel_hash(app.channel.id.as_str());
    assert!(
        logs.lines()
            .any(|line| line.contains("provider turn outcome")
                && line.contains("outcome=\"completed\"")
                && line.contains(REVIEWER)),
        "{logs}"
    );
    let rejected: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("automatic output rejected"))
        .collect();
    assert_eq!(rejected.len(), 1, "{logs}");
    assert!(
        rejected[0].contains("reason=\"default_reply_unavailable\"")
            && rejected[0].contains(REVIEWER)
            && rejected[0].contains(&channel_hash),
        "{logs}"
    );
    let leases: Vec<_> = (0..4)
        .map(|_| {
            app.capacity
                .try_lease(
                    &app.channel.pointer.record_id,
                    &app.channel.definition.owner.subject,
                )
                .unwrap()
        })
        .collect();
    drop(leases);
}
