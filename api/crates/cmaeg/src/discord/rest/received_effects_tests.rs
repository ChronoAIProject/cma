use super::Fixture;
use crate::discord::RestError;
use cmaeg_core::message_effect::MessageReactionRequest;
use reqwest::Method;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

fn request(emoji: &str) -> MessageReactionRequest {
    serde_json::from_value(json!({"reaction":[{"type":"emoji","emoji":emoji}]})).unwrap()
}
async fn fixture() -> Fixture {
    let f = Fixture::new().await;
    f.memory.lock().unwrap().message =
        json!({"id":"800","channel_id":"99","type":0,"author":{"id":"42","bot":false}});
    f
}
#[tokio::test]
async fn received_reactions_require_exact_original_human_author_bot_and_message() {
    for defect in 0..10 {
        let f = fixture().await;
        {
            let mut memory = f.memory.lock().unwrap();
            match defect {
                0 => memory.message["author"]["id"] = "43".into(),
                1 => memory.message["author"]["bot"] = true.into(),
                2 => memory.message["author"]["bot"] = "false".into(),
                3 => memory.message["webhook_id"] = "55".into(),
                4 => memory.message["type"] = 7.into(),
                5 => memory.message["channel_id"] = "100".into(),
                6 => memory.message["id"] = "801".into(),
                7 => memory.user["id"] = "78".into(),
                8 => memory.message["author"]["system"] = true.into(),
                9 => memory.message["type"] = Value::Null,
                _ => unreachable!(),
            }
        }
        assert_eq!(
            f.api
                .set_received_message_reaction(
                    &f.token,
                    "77",
                    "99",
                    "800",
                    "42",
                    &request("👍"),
                    || async { Ok(()) }
                )
                .await,
            Err(RestError::Rejected),
            "defect {defect}"
        );
        assert!(f.memory.lock().unwrap().writes.is_empty());
    }
}
#[tokio::test]
async fn received_reactions_check_custody_after_reads_and_before_each_partial_write() {
    for fail_check in [1, 3] {
        let f = fixture().await;
        f.memory.lock().unwrap().message["reactions"] = json!([
            {"me":true,"emoji":{"id":null,"name":"👍"}},
            {"me":false,"emoji":{"id":null,"name":"👀"}}
        ]);
        let checks = AtomicUsize::new(0);
        let outcome = f
            .api
            .set_received_message_reaction(
                &f.token,
                "77",
                "99",
                "800",
                "42",
                &request("❤️"),
                || async {
                    if checks.fetch_add(1, Ordering::SeqCst) + 1 == fail_check {
                        Err(RestError::Rejected)
                    } else {
                        Ok(())
                    }
                },
            )
            .await;
        assert_eq!(
            outcome,
            Err(if fail_check == 1 {
                RestError::Rejected
            } else {
                RestError::Unknown
            })
        );
        let memory = f.memory.lock().unwrap();
        assert_eq!(memory.writes.len(), usize::from(fail_check == 3));
        if fail_check == 3 {
            assert_eq!(memory.writes[0].0, Method::DELETE);
            assert!(memory.writes[0].1.ends_with("/@me"));
        }
    }
}
#[tokio::test]
async fn received_reactions_validate_all_own_reactions_before_mutation() {
    let f = fixture().await;
    f.memory.lock().unwrap().message["reactions"] = json!([
        {"me":true,"emoji":{"id":null,"name":"👍"}},
        {"me":true,"emoji":{"id":null,"name":".."}}
    ]);
    assert_eq!(
        f.api
            .set_received_message_reaction(
                &f.token,
                "77",
                "99",
                "800",
                "42",
                &request("❤️"),
                || async { Ok(()) }
            )
            .await,
        Err(RestError::Rejected)
    );
    assert!(f.memory.lock().unwrap().writes.is_empty());
}
