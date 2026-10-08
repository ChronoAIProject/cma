//! Release scenarios at real HTTP ingress, provider and delivery boundaries.
use super::{app::App, fake_provider::FakeProviderConfig, policy::REVIEWER, requests::event};
use serde_json::{json, Value};
use std::time::Duration;

pub async fn app(edit: impl FnOnce(&mut Value)) -> App {
    configured(true, edit).await
}

pub async fn configured(enabled: bool, edit: impl FnOnce(&mut Value)) -> App {
    let provider = |text: &str| FakeProviderConfig {
        terminal_delay: Duration::from_millis(500),
        text: text.into(),
        ..Default::default()
    };
    App::configured(
        enabled,
        enabled,
        [provider("main answer"), provider("review answer")],
        |raw| {
            raw["reply"]["ack_after_ms"] = 1.into();
            raw["routing"]["default_consumer"]["label"] = "Main".into();
            raw["routing"]["consumers"][0]["label"] = "Reviewer".into();
            edit(raw);
        },
    )
    .await
}

pub fn texts(app: &App) -> Vec<String> {
    app.peers
        .chat_calls()
        .into_iter()
        .map(|(method, body)| {
            assert_eq!(method, "sendMessage");
            body["text"].as_str().unwrap().to_owned()
        })
        .collect()
}

#[tokio::test]
async fn mentions_and_commands_deliver_one_ack_and_each_selected_label() {
    for (text, expected, counts) in [
        ("@reviewer hi", vec!["[Reviewer] review answer"], [0, 1]),
        (
            "/to reviewer,main hi",
            vec!["[Main] main answer", "[Reviewer] review answer"],
            [1, 1],
        ),
    ] {
        let mut app = app(|_| {}).await;
        let response = app.native(text, 901).await;
        assert_eq!(response.status(), 202);
        let response: Value = response.json().await.unwrap();
        assert_eq!(response["status"], "accepted");
        app.drain().await;
        assert_eq!([app.turns(0).len(), app.turns(1).len()], counts);
        let mut sent = texts(&app);
        assert_eq!(sent.remove(0), "Working on it…");
        sent.sort();
        assert_eq!(sent, expected);
        for turn in app.turns(0).into_iter().chain(app.turns(1)) {
            let input = turn["input"][0]["content"][0]["text"].as_str().unwrap();
            assert!(input.ends_with("hi"), "{input}");
            assert!(!input.contains(text), "routing prefix reached provider");
        }
        println!("{text}: one ack, replies={sent:?}, provider_turns={counts:?}");
    }
}

#[tokio::test]
async fn unknown_mention_stays_text_and_uses_rules_but_unknown_command_gets_one_notice() {
    let mut app = app(|raw| {
        raw["routing"]["default_targets"] = json!([REVIEWER]);
        raw["routing"]["rules"] = json!([{"id":"fallback","when":{},"targets":["default"]}]);
    })
    .await;
    let response = app.native("@x hi", 902).await;
    assert_eq!(response.status(), 202);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["status"], "accepted");
    app.drain().await;
    assert_eq!(app.turns(0).len(), 1);
    assert!(app.turns(1).is_empty());
    let turn = &app.turns(0)[0];
    assert!(turn["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .ends_with("@x hi"));
    let route = &turn["event_context"]["activity"]["routing"];
    assert_eq!(route["resolution"], "rule");
    assert_eq!(route["rule_id"], "fallback");
    assert_eq!(texts(&app), ["Working on it…", "[Main] main answer"]);
    println!("@x hi: raw text preserved, rule=fallback, one Main reply");

    let mut app = self::app(|_| {}).await;
    let response = app.native("/to ghost", 903).await;
    assert_eq!(response.status(), 202);
    let response: Value = response.json().await.unwrap();
    assert_eq!(
        response,
        json!({"status":"refused","reason":"target_unknown"})
    );
    app.drain().await;
    assert!(app.turns(0).is_empty() && app.turns(1).is_empty());
    assert_eq!(
        texts(&app),
        ["No agent is named ghost. Available: main, reviewer."]
    );
    println!("/to ghost: one notice, no provider turn");
}

#[tokio::test]
async fn custom_multi_target_accepts_each_consumer_and_delivers_each_terminal() {
    let mut app = app(|_| {}).await;
    let response = app
        .event(
            &app.channel,
            event("custom-smoke", Some("reviewer,main")),
            false,
        )
        .await;
    assert_eq!(response.status(), 202);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["consumers"].as_array().unwrap().len(), 2);
    assert_eq!(response["consumers"][0]["consumer_id"], REVIEWER);
    assert_eq!(response["consumers"][1]["consumer_id"], "default");
    assert_eq!(
        response["conversation_id"],
        response["consumers"][0]["conversation_id"]
    );
    app.drain().await;
    assert_eq!([app.turns(0).len(), app.turns(1).len()], [1, 1]);
    assert!(app.peers.chat_calls().is_empty());
    let calls = app.peers.calls.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].1["id"], calls[1].1["id"]);
    for consumer in response["consumers"].as_array().unwrap() {
        assert_eq!(consumer["status"], "accepted");
        let matching: Vec<_> = calls
            .iter()
            .filter(|(kind, body)| {
                kind == "webhook" && body["data"]["conversation_id"] == consumer["conversation_id"]
            })
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].1["type"], "cmaeg.response.completed");
        assert_eq!(matching[0].1["subject"], "custom-smoke");
        assert_eq!(matching[0].1["data"]["event_id"], "custom-smoke");
        assert_eq!(
            matching[0].1["data"]["consumer"]["id"],
            consumer["consumer_id"]
        );
        assert_eq!(matching[0].1["data"]["consumer"]["name"], consumer["name"]);
        assert_eq!(
            matching[0].1["data"]["consumer"]["label"],
            if consumer["consumer_id"] == "default" {
                "Main"
            } else {
                "Reviewer"
            }
        );
    }
    println!(
        "custom reviewer,main: 202, ordered admissions, one completed webhook per conversation"
    );
}
