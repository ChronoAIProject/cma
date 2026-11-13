use super::*;
use crate::discord::{
    journal_fixture::{Fixture, TOKEN},
    status::{Stage, StatusRegistry},
    NativeDiscordJournal,
};
use std::{
    io,
    sync::{Arc, Mutex},
};

fn discovery() -> Discovery {
    Discovery {
        url: "wss://gateway.discord.gg/".into(),
        shards: 1,
        session_start_limit: SessionLimit {
            total: 10,
            remaining: 10,
            reset_after: 86_400_000,
            max_concurrency: 1,
        },
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn each_predicate_logs_only_fixed_reason_and_bounded_numbers_without_parent_identity() {
    static KEEP: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    KEEP.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    for (field, value, reason, refused) in [
        ("total", 0, "total_zero", true),
        ("remaining", 11, "remaining_gt_total", true),
        ("max_concurrency", 0, "max_concurrency_zero", true),
        ("reset_after", 0, "reset_after_zero", false),
        ("reset_after", u64::MAX, "reset_after_gt_7d", false),
    ] {
        let mut d = discovery();
        match field {
            "total" => {
                d.session_start_limit.total = value;
                d.session_start_limit.remaining = 0;
            }
            "remaining" => d.session_start_limit.remaining = value,
            "max_concurrency" => d.session_start_limit.max_concurrency = value,
            _ => d.session_start_limit.reset_after = value,
        }
        let capture = Capture::default();
        let sink = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let parent = tracing::info_span!(
                "sensitive",
                owner = "private-owner",
                token = "private-token",
                session = "private-session",
                body = "private-body"
            );
            let _entered = parent.enter();
            let registry = StatusRegistry::default();
            let status = registry.begin("private-record", Some("private-revision"));
            status.step(Stage::Discovery);
            assert_eq!(validate(&d, &status).is_err(), refused);
            for _ in 0..100 {
                let _ = validate(&d, &status);
            }
        });
        let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        let line = logs
            .lines()
            .find(|s| s.contains("native Discord session limit observed"))
            .unwrap();
        assert!(line.contains(&format!("reason=\"{reason}\"")), "{line}");
        assert!(line.contains(&format!("refused={refused}")), "{line}");
        for (key, number) in [
            ("shards", d.shards),
            ("total", d.session_start_limit.total),
            ("remaining", d.session_start_limit.remaining),
            ("reset_after", d.session_start_limit.reset_after),
            ("max_concurrency", d.session_start_limit.max_concurrency),
        ] {
            assert!(line.contains(&format!("{key}={number}")), "{line}");
        }
        assert!(logs.lines().count() <= 64);
        for forbidden in ["private-", "sensitive", "gateway.discord.gg", TOKEN] {
            assert!(!logs.contains(forbidden));
        }
    }
}

#[test]
fn documented_reset_boundaries_are_accepted_without_timestamp_overflow() {
    for reset in [0, 1, 604_800_000, 604_800_001, i64::MAX as u64, u64::MAX] {
        let mut d = discovery();
        d.session_start_limit.reset_after = reset;
        assert_eq!(validate(&d, &StatusAttempt::default()), Ok(()));
        let deadline = d.session_start_limit.reset_deadline(1_000);
        assert!(deadline >= 1_000);
        if reset == 0 || reset >= i64::MAX as u64 {
            assert_eq!(deadline, i64::MAX);
        } else {
            assert_eq!(deadline, 1_000 + reset as i64);
        }
    }
}

#[tokio::test]
async fn invalid_limits_never_acquire_a_lease_and_auth_failures_stay_definitive() {
    for (field, value) in [("total", 0), ("remaining", 11), ("max_concurrency", 0)] {
        let f = Fixture::new().await;
        f.memory.gateway.lock().unwrap()["session_start_limit"][field] = value.into();
        assert!(matches!(
            f.acquire().await,
            Err(GatewayError::SessionLimitUnavailable)
        ));
        assert!(f.memory.rows.lock().unwrap().is_empty());
    }
    let f = Fixture::new().await;
    f.memory.user.lock().unwrap()["id"] = "99".into();
    assert!(matches!(
        f.acquire().await,
        Err(GatewayError::Authentication)
    ));
    assert!(f.memory.rows.lock().unwrap().is_empty());
    assert!(matches!(
        f.api
            .discover(&cmaeg_core::channel::Secret::new("wrong-fixture"), "77")
            .await,
        Err(GatewayError::Authentication)
    ));
}

#[tokio::test]
async fn zero_reset_never_refunds_identify_and_positive_window_recovers_conservatively() {
    let f = Fixture::new().await;
    {
        let mut response = f.memory.gateway.lock().unwrap();
        response["session_start_limit"]["remaining"] = 1.into();
        response["session_start_limit"]["reset_after"] = 0.into();
    }
    let first = f.acquire().await.unwrap();
    first.authorize_connection(None).await.unwrap();
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    first.release().await.unwrap();
    f.advance(5_001);
    // Stale upstream still says one remaining. Durable consumption wins.
    let second = f.acquire().await.unwrap();
    assert_eq!(
        second.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
    f.memory.gateway.lock().unwrap()["session_start_limit"]["reset_after"] = 10_000.into();
    assert_eq!(
        second.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
    second.release().await.unwrap();
    f.advance(9_999);
    let third = f.acquire().await.unwrap();
    assert_eq!(
        third.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
    f.advance(1);
    third.authorize_connection(None).await.unwrap();
    third.release().await.unwrap();
    let fourth = f.acquire().await.unwrap();
    assert_eq!(
        fourth.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
}

#[tokio::test]
async fn long_reset_keeps_one_lease_and_five_second_identify_bucket() {
    let f = Fixture::new().await;
    f.memory.gateway.lock().unwrap()["session_start_limit"]["reset_after"] = u64::MAX.into();
    let first = f.acquire().await.unwrap();
    first.authorize_connection(None).await.unwrap();
    assert!(matches!(f.acquire().await, Err(GatewayError::Authority)));
    first.release().await.unwrap();
    let second = f.acquire().await.unwrap();
    assert_eq!(
        second.authorize_connection(None).await,
        Err(GatewayError::RateLimited)
    );
    f.advance(5_000);
    second.authorize_connection(None).await.unwrap();
}
