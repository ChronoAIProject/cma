use super::support::App;
use cmaeg::{auth::AgentCaller, delivery::intents::DeliveryOutcome, CommonmarkRelay};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::time::Instant;

#[tokio::test]
async fn complete_relay_tool_waits_for_receipt_but_keeps_a_finite_owning_deadline() {
    for (enabled, delay, expected_elapsed, confirmed) in [
        (false, 9, 8, false),
        (true, 12, 12, true),
        (true, 19, 18, false),
    ] {
        check_wait(
            "telegram",
            enabled,
            CommonmarkRelay::default(),
            delay,
            expected_elapsed,
            confirmed,
        )
        .await;
    }
}

#[tokio::test]
async fn commonmark_tool_wait_extends_only_telegram_replies_and_keeps_its_deadline() {
    for (platform, commonmark, delay, elapsed, confirmed) in [
        ("telegram", false, 9, 8, false),
        ("telegram", true, 12, 12, true),
        ("telegram", true, 19, 18, false),
        ("feishu", true, 9, 8, false),
        ("lark", true, 9, 8, false),
    ] {
        let platforms = CommonmarkRelay {
            telegram: commonmark,
            ..Default::default()
        };
        check_wait(platform, false, platforms, delay, elapsed, confirmed).await;
    }
}

async fn check_wait(
    platform: &str,
    enabled: bool,
    commonmark: CommonmarkRelay,
    delay: u64,
    expected_elapsed: u64,
    confirmed: bool,
) {
    let mut app = if platform == "telegram" {
        App::new().await
    } else {
        App::on(platform).await
    };
    app.state.complete_replies_enabled = enabled;
    app.state.commonmark_relay = commonmark;
    let caller = AgentCaller {
        key_hash: Sha256::digest(app.key.as_bytes()).into(),
    };
    // Resolve/authenticate before pausing. The actual reply rechecks the
    // same cached authority, then uses the fixture's in-process engine.
    app.state
        .authorize_event(caller, &app.event_ref, "replyToEvent")
        .await
        .unwrap();
    app.delay.store(delay * 1000, Ordering::SeqCst);
    tokio::time::pause();
    let started = Instant::now();
    let result = app
        .state
        .reply(
            caller,
            &app.event_ref,
            serde_json::from_value(json!({"text":"x".repeat(9000)})).unwrap(),
        )
        .await;
    // Pausing after authority IO can leave the timer driver between
    // millisecond ticks; allow that rounding at the owning deadline.
    let expected_elapsed = Duration::from_secs(expected_elapsed);
    assert!(
        (expected_elapsed..=expected_elapsed + Duration::from_millis(2))
            .contains(&started.elapsed()),
        "enabled={enabled}, delay={delay}: {:?}",
        started.elapsed()
    );
    if confirmed {
        assert!(matches!(
            result.unwrap().delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert_eq!(app.requests.lock().unwrap().len(), 1);
    } else {
        let problem = result.unwrap_err();
        assert_eq!(problem.status, 504);
        assert_eq!(problem.detail.as_deref(), Some("delivery_timeout"));
        assert!(app.requests.lock().unwrap().is_empty());
    }
    drop(app);
    tokio::time::resume();
}
