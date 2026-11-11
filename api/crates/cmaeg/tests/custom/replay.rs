//! Exercise authenticated HTTP ingress, the real runner and exact provider admission.
use super::{fake_provider::FakeProviderConfig, support::App};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn exact_provider() -> FakeProviderConfig {
    FakeProviderConfig {
        exact_admission: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn legacy_nyxbot_ingress_does_not_require_the_optional_context_reader() {
    let mut app = App::new(
        |value| {
            value["provider"]["slug"] = "nyxbot".into();
            value["profile"]["metadata"]["provider"] = "nyxbot".into();
        },
        FakeProviderConfig {
            context_status: Some(405),
            ..Default::default()
        },
        Default::default(),
        0,
    )
    .await;
    let response = app
        .request(&app.url, app.event("legacy-nyxbot").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    app.drain().await;
    assert_eq!(app.provider.requests().len(), 1);
    assert!(app
        .provider
        .log()
        .iter()
        .all(|request| request.method != "GET"));
    let results = app.receiver.events();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "cmaeg.response.completed");
}

#[tokio::test]
async fn completed_event_retries_preserve_admission_context_output_and_waiter_ownership() {
    let mut app = App::new(|_| {}, exact_provider(), Default::default(), 0).await;
    let event = app.event("completed-replay").to_string();
    let stream_url = format!("{}?mode=stream", app.url);
    let first = app
        .request(&stream_url, event.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    assert!(first
        .text()
        .await
        .unwrap()
        .contains("event: cmaeg.completed"));
    let original = app.provider.admitted();
    assert_eq!(original.len(), 1);
    for url in [&app.url, &stream_url] {
        let replay = app.request(url, event.clone()).send().await.unwrap();
        assert_eq!(replay.status(), 202);
        let accepted: Value = replay.json().await.unwrap();
        assert_eq!(accepted["event_id"], "completed-replay");
    }
    app.drain().await;
    assert_eq!(app.provider.admitted(), original);
    assert_eq!(app.provider.requests().len(), 1);
    assert!(app.waiters.is_empty());
    let results = app.receiver.events();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "cmaeg.response.completed");
    assert_eq!(results[0]["data"]["text"], "hello");
}

#[tokio::test]
async fn retry_after_losing_sync_response_keeps_original_turn_and_output() {
    let mut config = exact_provider();
    config.terminal_delay = Duration::from_millis(250);
    let mut app = App::new(|_| {}, config, Default::default(), 0).await;
    let event = app.event("lost-response").to_string();
    let url = format!("{}?mode=stream", app.url);
    let original = app.request(&url, event.clone()).send().await.unwrap();
    assert_eq!(original.status(), 200);
    drop(original);
    let retry = app.request(&url, event).send().await.unwrap();
    assert_eq!(retry.status(), 202);
    app.drain().await;
    assert_eq!(app.provider.requests().len(), 1);
    assert_eq!(app.provider.admitted().len(), 1);
    let results = app.receiver.events();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "cmaeg.response.completed");
    assert_eq!(results[0]["data"]["text"], "hello");
}

#[tokio::test]
async fn changed_event_content_is_a_conflict_without_overwriting_original_result() {
    for field in [
        "data",
        "source",
        "time",
        "subject",
        "extension",
        "attachments",
    ] {
        let mut app = App::new(|_| {}, exact_provider(), Default::default(), 0).await;
        let mut event = app.event("changed-replay");
        event["time"] = "2026-10-28T01:00:00Z".into();
        let first = app
            .request(&format!("{}?mode=stream", app.url), event.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        first.text().await.unwrap();
        let original = app.provider.admitted();
        match field {
            "data" => event["data"]["a"] = 3.into(),
            "source" => event["source"] = "https://different.example".into(),
            "time" => event["time"] = "2026-10-28T02:00:00Z".into(),
            "subject" => event["subject"] = "different-subject".into(),
            "extension" => event["revision"] = "second".into(),
            "attachments" => {
                event["data"]["attachments"] = json!([{"url":"https://files.example/new.txt"}])
            }
            _ => unreachable!(),
        }
        let retry = app
            .request(&app.url, event.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(retry.status(), 409, "changed {field}");
        assert_eq!(retry.json::<Value>().await.unwrap()["code"], "conflict");
        app.drain().await;
        assert_eq!(app.provider.admitted(), original);
        assert_eq!(app.provider.requests().len(), 1);
        assert_eq!(app.receiver.events().len(), 1);
    }
}

#[tokio::test]
async fn concurrent_first_requests_recover_the_winner_and_keep_only_its_stream() {
    for (changed, retry_enabled) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut config = exact_provider();
        config.admission_barrier = Some(Arc::new(tokio::sync::Barrier::new(2)));
        if retry_enabled {
            // Both preflights see absence; retry only the losing POST's
            // retained-context reconciliation, without another submission.
            config.context_steps = vec![
                (Duration::ZERO, None, None),
                (Duration::ZERO, None, None),
                (Duration::ZERO, Some(503), None),
            ];
        }
        let mut app = App::new(
            |_| {},
            config,
            cmaeg::TurnConfig {
                context_retry_enabled: retry_enabled,
                ..Default::default()
            },
            0,
        )
        .await;
        let event = app.event("concurrent-replay");
        let mut second = event.clone();
        if changed {
            second["data"]["a"] = 999.into();
        }
        let url = format!("{}?mode=stream", app.url);
        let (first, second) = tokio::join!(
            app.request(&url, event.to_string()).send(),
            app.request(&url, second.to_string()).send(),
        );
        let responses = [first.unwrap(), second.unwrap()];
        let mut statuses = responses
            .iter()
            .map(|r| r.status().as_u16())
            .collect::<Vec<_>>();
        statuses.sort();
        assert_eq!(
            statuses,
            if changed {
                vec![200, 409]
            } else {
                vec![200, 202]
            }
        );
        for response in responses {
            if response.status() == 200 {
                assert!(response
                    .text()
                    .await
                    .unwrap()
                    .contains("event: cmaeg.completed"));
            }
        }
        app.drain().await;
        assert_eq!(app.provider.requests().len(), 2);
        assert_eq!(app.provider.admitted().len(), 1);
        assert_eq!(
            app.provider
                .log()
                .iter()
                .filter(|r| r.method == "GET")
                .count(),
            if retry_enabled { 4 } else { 3 }
        );
        assert_eq!(app.receiver.events().len(), 1);
        assert!(app.waiters.is_empty());
    }
}

#[tokio::test]
async fn authority_and_unavailable_lookup_failures_do_not_submit_or_send_failed_results() {
    for (status, code) in [
        (401, "not_authenticated"),
        (403, "forbidden"),
        (503, "provider_unavailable"),
    ] {
        let mut config = exact_provider();
        config.context_status = Some(status);
        config.context_delay = Duration::from_millis(60);
        let mut app = App::new(
            |_| {},
            config,
            cmaeg::TurnConfig {
                accept_budget: Duration::from_millis(20),
                ..Default::default()
            },
            0,
        )
        .await;
        let result = app
            .request(&app.url, app.event("lookup-refused").to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(result.status().as_u16(), status);
        assert_eq!(result.json::<Value>().await.unwrap()["code"], code);
        app.drain().await;
        assert!(app.provider.requests().is_empty());
        assert!(app.provider.admitted().is_empty());
        assert!(app.receiver.events().is_empty());
    }
}

#[tokio::test]
async fn provider_context_from_another_channel_or_conversation_never_acknowledges_replay() {
    for patch in [
        json!({"channel_id":"different-channel"}),
        json!({"conversation_id":"conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
    ] {
        let mut config = exact_provider();
        config.context_override = Some(patch);
        let mut app = App::new(|_| {}, config, Default::default(), 0).await;
        let event = app.event("wrong-identity").to_string();
        let first = app
            .request(&format!("{}?mode=stream", app.url), event.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        first.text().await.unwrap();
        let retry = app.request(&app.url, event).send().await.unwrap();
        assert_eq!(retry.status(), 503);
        app.drain().await;
        assert_eq!(app.provider.requests().len(), 1);
        assert_eq!(app.receiver.events().len(), 1);
    }
}

#[tokio::test]
async fn extracted_attachments_are_preserved_and_changed_attachments_conflict() {
    let mut app = App::media(
        |value| {
            value["sources"][0]["attachments"] =
                json!({"pointer":"/files","origins":["https://files.example"]});
        },
        true,
    )
    .await;
    let mut event = app.event("attachment-replay");
    event["data"]["files"] =
        json!([{"url":"https://files.example/original.txt","mime_type":"text/plain"}]);
    let first = app
        .request(&format!("{}?mode=stream", app.url), event.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    first.text().await.unwrap();
    let original = app.provider.admitted();
    let body: Value = serde_json::from_str(&original[0].body).unwrap();
    assert_eq!(
        body["event_context"]["activity"]["kind"]["attachments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let replay = app
        .request(&app.url, event.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 202);
    event["data"]["files"][0]["url"] = "https://files.example/replacement.txt".into();
    let changed = app
        .request(&app.url, event.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), 409);
    app.drain().await;
    assert_eq!(app.provider.admitted(), original);
    assert_eq!(app.provider.requests().len(), 1);
    assert_eq!(app.receiver.events().len(), 1);
}

#[tokio::test]
async fn concurrent_admission_beyond_acceptance_budget_does_not_open_unowned_streams() {
    let mut config = exact_provider();
    config.header_delay = Duration::from_millis(100);
    config.admission_barrier = Some(Arc::new(tokio::sync::Barrier::new(2)));
    let mut app = App::new(
        |_| {},
        config,
        cmaeg::TurnConfig {
            accept_budget: Duration::from_millis(20),
            ..Default::default()
        },
        0,
    )
    .await;
    let event = app.event("slow-concurrent-replay").to_string();
    let url = format!("{}?mode=stream", app.url);
    let (first, second) = tokio::join!(
        app.request(&url, event.clone()).send(),
        app.request(&url, event).send(),
    );
    for response in [first.unwrap(), second.unwrap()] {
        assert_eq!(response.status(), 202);
        assert!(response.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("application/json"));
    }
    assert!(app.waiters.is_empty());
    app.drain().await;
    assert_eq!(app.provider.admitted().len(), 1);
    assert_eq!(app.provider.requests().len(), 2);
    let results = app.receiver.events();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "cmaeg.response.completed");
}
