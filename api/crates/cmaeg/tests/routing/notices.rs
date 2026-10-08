//! Refusal notices use the ordinary bounded scheduler and transport renderer.
use super::{routing_fixture::REVIEWER, support::*};
use cmaeg::delivery::{
    custom::CustomDelivery, engine::DeliveryEngine, intents::DeliveryCommand,
    relay_client::RelayClient,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

async fn refused(response: reqwest::Response, reason: &str) {
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"status":"refused","reason":reason})
    );
}

#[tokio::test]
async fn unknown_target_is_sent_once_escaped_and_bounded_by_the_existing_engine() {
    let mut app = App::new(true, |_| {}).await;
    let token = format!("https://x.example/{}", "界".repeat(40));
    refused(
        app.native(&native(&format!("/to {token}"))).await,
        "target_unknown",
    )
    .await;
    assert!(app.recorder.requests.lock().unwrap().is_empty());
    let relay = RelayClient::new(&app.platform_base, Duration::from_secs(1))
        .unwrap()
        .with_telegram(
            cmaeg::telegram::TelegramClient::new(&app.platform_base, Duration::from_secs(1))
                .unwrap(),
        );
    let engine = DeliveryEngine::start(
        app.outcomes.take().unwrap(),
        app.state.master_keys.clone(),
        relay,
        Default::default(),
        app.tracker.clone(),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
    );
    assert!(engine.drain(deadline()).await);
    let calls = app.platform.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "sendMessage");
    let body = &calls[0].1;
    assert_eq!(body["chat_id"], "42");
    assert!(body.get("parse_mode").is_none());
    let text = body["text"].as_str().unwrap();
    assert!(
        text.contains("https\\[:\\]\\[/\\]\\[/\\]x\\[.\\]example\\[/\\]"),
        "{text}"
    );
    assert_eq!(
        text.matches('界').count(),
        32 - "https://x.example/".chars().count()
    );
    assert!(!text.contains("https://"));
    assert!(!text.contains("x.example"));
    assert!(!text.contains("\\\\["));
    assert_eq!(
        app.observer.refused.lock().unwrap()[0].code,
        "target_unknown"
    );
}

#[tokio::test]
async fn hidden_and_pending_consumers_are_not_advertised_in_notices() {
    let mut app = App::new(true, |raw| {
        raw["routing"]["consumers"][0]["mentionable"] = false.into();
        raw["routing"]["consumers"][1]["pending"] = true.into();
    })
    .await;
    for token in [REVIEWER, "REVIEWER"] {
        refused(
            app.native(&native(&format!("/to {token}"))).await,
            "target_not_mentionable",
        )
        .await;
        let DeliveryCommand::Notice { text, .. } =
            app.outcomes.as_mut().unwrap().try_recv().unwrap()
        else {
            panic!("expected notice")
        };
        assert_eq!(text, format!("No agent is named {token}. Available: main."));
    }
    assert!(app.recorder.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn target_limit_uses_fixed_text_and_notice_policy_can_disable_it() {
    for enabled in [true, false] {
        let mut app = App::new(true, |raw| {
            raw["routing"]["max_targets"] = 1.into();
            raw["routing"]["addressing"] = json!({"unknown_target_notice":enabled});
        })
        .await;
        refused(
            app.relay(&envelope("/to reviewer,main")).await,
            "too_many_targets",
        )
        .await;
        let queued = app.outcomes.as_mut().unwrap().try_recv();
        if enabled {
            let DeliveryCommand::Notice { text, address, .. } = queued.unwrap() else {
                panic!("expected notice")
            };
            assert_eq!(text, "Too many agents addressed; at most 1 per message.");
            assert!(matches!(
                address,
                cmaeg_core::seal::ReplyAddress::NyxidRelay { reply_token, .. } if !reply_token.as_str().is_empty()
            ));
        } else {
            assert!(queued.is_err());
        }
    }
}

#[test]
fn full_and_closed_queues_drop_notices_without_waiting_and_count_each_reason() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut app = App::new(true, |_| {}).await;
                refused(app.native(&native("/to ghost")).await, "target_unknown").await;
                let response = tokio::time::timeout(
                    Duration::from_millis(500),
                    app.native(&native("/to another")),
                )
                .await
                .unwrap();
                refused(response, "target_unknown").await;
                assert!(matches!(
                    app.outcomes.as_mut().unwrap().try_recv(),
                    Ok(DeliveryCommand::Notice { .. })
                ));
                assert!(app.outcomes.as_mut().unwrap().try_recv().is_err());
                drop(app.outcomes.take());
                refused(app.native(&native("/to last")).await, "target_unknown").await;
            });
    });
    let rendered = handle.render();
    assert!(
        rendered.contains("cmaeg_routing_notices_dropped_total{reason=\"queue_full\"} 1"),
        "{rendered}"
    );
    assert!(
        rendered.contains("cmaeg_routing_notices_dropped_total{reason=\"closed\"} 1"),
        "{rendered}"
    );
}
