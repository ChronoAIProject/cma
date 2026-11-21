use super::{
    media_fixture::{Answer, Fixture},
    Error,
};
use cmaeg_core::{channel::Secret, message::StructuredContent};
use serde_json::{json, Value};

pub(crate) fn samples() -> Vec<(&'static str, Value)> {
    vec![
        (
            "sendContact",
            json!({"type":"contact","phone_number":"+6512345678","first_name":"Alice","last_name":"Example","vcard":"BEGIN:VCARD\r\nEND:VCARD"}),
        ),
        (
            "sendLocation",
            json!({"type":"location","latitude":1.3,"longitude":103.8,"horizontal_accuracy":2.5}),
        ),
        (
            "sendVenue",
            json!({"type":"venue","latitude":1.3,"longitude":103.8,"title":"Library","address":"Main Street"}),
        ),
    ]
}
pub(crate) fn result(content: &Value, chat: i64, thread: Option<i64>) -> Value {
    let mut value = json!({"message_id":800,"chat":{"id":chat},"from":{"id":77,"is_bot":true}});
    value[content["type"].as_str().unwrap()] = content.clone();
    if let Some(thread) = thread {
        value["message_thread_id"] = thread.into();
    }
    value
}
#[tokio::test]
async fn static_replies_use_official_methods_and_only_explicit_original_chat_topic() {
    for (method, content) in samples() {
        for thread in [None, Some("7")] {
            let f = Fixture::new().await;
            f.answer(Answer::json(
                reqwest::StatusCode::OK,
                json!({"ok":true,"result":result(&content,-42,thread.map(|_|7))}),
            ));
            let value: StructuredContent = serde_json::from_value(content.clone()).unwrap();
            assert_eq!(
                f.api
                    .send_structured(&Secret::new("77:fixture"), "77", "-42", thread, &value)
                    .await
                    .unwrap(),
                "800"
            );
            let calls = f.calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].path, format!("/bot77:fixture/{method}"));
            let mut expected = content.clone();
            expected.as_object_mut().unwrap().remove("type");
            expected["chat_id"] = "-42".into();
            if let Some(thread) = thread {
                expected["message_thread_id"] = thread.into();
            }
            assert_eq!(
                serde_json::from_slice::<Value>(&calls[0].body).unwrap(),
                expected
            );
            assert!(!String::from_utf8_lossy(&calls[0].body).contains("77:fixture"));
        }
    }
}
#[tokio::test]
async fn mismatched_or_uncertain_receipts_and_rate_limits_never_retry() {
    for case in [
        "chat", "thread", "sender", "human", "content", "live", "server", "rate",
    ] {
        let f = Fixture::new().await;
        let content = samples().remove(1).1;
        let mut value = result(&content, -42, Some(7));
        match case {
            "chat" => value["chat"]["id"] = (-43).into(),
            "thread" => value["message_thread_id"] = 8.into(),
            "sender" => value["from"]["id"] = 78.into(),
            "human" => value["from"]["is_bot"] = false.into(),
            "content" => value["location"] = Value::Null,
            "live" => value["location"]["live_period"] = 60.into(),
            _ => {}
        }
        let status = if case == "server" {
            reqwest::StatusCode::BAD_GATEWAY
        } else if case == "rate" {
            reqwest::StatusCode::TOO_MANY_REQUESTS
        } else {
            reqwest::StatusCode::OK
        };
        f.answer(Answer::json(
            status,
            json!({"ok":true,"result":value,"parameters":{"retry_after":2}}),
        ));
        let outcome = f
            .api
            .send_structured(
                &Secret::new("77:fixture"),
                "77",
                "-42",
                Some("7"),
                &serde_json::from_value(content).unwrap(),
            )
            .await;
        assert_eq!(
            outcome,
            Err(if case == "rate" {
                Error::RateLimited(2)
            } else {
                Error::Unconfirmed
            }),
            "{case}"
        );
        assert_eq!(f.calls().len(), 1);
    }
}
