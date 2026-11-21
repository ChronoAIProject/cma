use super::{fixture::Fixture, *};
use cmaeg_core::channel::Secret;
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn discord_confirms_only_204_and_never_retries_a_write() {
    for status in [
        StatusCode::OK,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::BAD_GATEWAY,
        StatusCode::FORBIDDEN,
    ] {
        let f = Fixture::discord(false, false).await;
        f.discord.memory.lock().unwrap().reject_write = Some((1, status));
        let error = f.typing().await.unwrap_err();
        assert_eq!(
            error.status,
            match status {
                StatusCode::TOO_MANY_REQUESTS => 429,
                StatusCode::FORBIDDEN => 403,
                _ => 503,
            }
        );
        assert_eq!(f.writes(), 1);
    }
}

#[tokio::test]
async fn discord_revalidates_native_bot_and_original_human_before_each_write() {
    for change in 0..5 {
        let f = Fixture::discord(false, false).await;
        f.typing().await.unwrap();
        {
            let mut memory = f.discord.memory.lock().unwrap();
            match change {
                0 => memory.user["id"] = "78".into(),
                1 => memory.message["id"] = "801".into(),
                2 => memory.message["author"]["bot"] = true.into(),
                3 => memory.message["author"]["id"] = "41".into(),
                4 => memory.message["webhook_id"] = "44".into(),
                _ => unreachable!(),
            }
        }
        assert!(f.typing().await.is_err());
        assert_eq!(f.writes(), 1);
    }
}

#[tokio::test]
async fn whatsapp_confirms_only_success_true_and_refuses_wrong_app_without_write() {
    for (status, body, expected) in [
        (StatusCode::OK, json!({"success":false}), 503),
        (StatusCode::OK, json!({}), 503),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":130429}}),
            429,
        ),
        (StatusCode::FORBIDDEN, json!({"error":{"code":190}}), 403),
        (StatusCode::BAD_GATEWAY, json!({"error":{"code":1}}), 503),
    ] {
        let f = Fixture::whatsapp(false, 1).await;
        *f.graph.response.lock().unwrap() = (status, body);
        assert_eq!(f.typing().await.unwrap_err().status, expected);
        assert_eq!(f.writes(), 1);
    }
    let f = Fixture::whatsapp(false, 1).await;
    *f.graph.app.lock().unwrap() = "99999".into();
    assert!(f.typing().await.is_err());
    assert_eq!(f.writes(), 0);
}

#[tokio::test]
async fn native_typing_timeout_cancels_and_does_not_continue_or_retry_in_background() {
    let f = Fixture::whatsapp(false, 1).await;
    *f.graph.delay.lock().unwrap() = Duration::from_secs(30);
    let started = std::time::Instant::now();
    assert_eq!(f.typing().await.unwrap_err().status, 503);
    assert!(started.elapsed() < Duration::from_secs(9));
    assert_eq!(f.writes(), 1);
    *f.graph.delay.lock().unwrap() = Duration::ZERO;
    f.typing().await.unwrap(); // Only the caller's explicit fresh request retries.
    assert_eq!(f.writes(), 2);
}

#[tokio::test]
async fn explicit_typing_capacity_refuses_immediately_without_platform_io() {
    let f = Fixture::whatsapp(false, 1).await;
    let client = f.app.state.native_typing.as_ref().unwrap();
    let _all = client.capacity.acquire_many(32).await.unwrap();
    assert_eq!(f.typing().await.unwrap_err().status, 429);
    assert!(f.graph.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn platform_transports_reject_untrusted_destination_ids() {
    let f = Fixture::whatsapp(false, 1).await;
    let credentials = crate::whatsapp::tests::credentials();
    assert!(f
        .graph
        .client()
        .typing(&credentials, "../other", "wamid.original")
        .await
        .is_err());
    assert!(f
        .discord
        .api
        .typing(
            &Secret::new("discord-effects-fixture"),
            "77",
            "../other",
            "800",
            "42",
            || async { Ok(()) }
        )
        .await
        .is_err());
    assert_eq!(f.writes(), 0);
}
