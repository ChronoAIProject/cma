//! Actual signed HTTP ingress, production admission constructor and Mongo.
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use layer2_core::sandbox::store::AgentStore;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "http/fixture.rs"]
mod fixture;
use fixture::*;
#[path = "http/races.rs"]
mod races;
#[path = "http/recovery.rs"]
mod recovery;
#[path = "http/recovery_gates.rs"]
mod recovery_gates;
#[path = "http/recovery_race.rs"]
mod recovery_race;
#[path = "http/runtime.rs"]
mod runtime;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_signed_cancel_and_steer_use_original_source_and_no_message_output() {
    let c = ControlFixture::with_callback_route().await;
    let before =
        c.f.resources
            .get_agent(&c.f.command.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
    for steer in [false, true] {
        let value = body(
            &c,
            if steer { "http-steer" } else { "http-cancel" },
            500 + u64::from(steer),
            steer,
        );
        let (status, result) = post(app(&c, true, None).await, &c, &value).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{result}");
        assert_eq!(result["status"], "control_accepted");
        assert_eq!(result["receipt"]["version"], 2);
        for key in ["completed", "cancelled", "command_id", "response_id"] {
            assert!(result["receipt"].get(key).is_none());
        }
    }
    assert_eq!(
        c.f.resources.get_agent(&before.id).await.unwrap().unwrap(),
        before
    );
    no_message_effects(&c, 2).await;
    assert_eq!(
        c.f.call("read_channel_control_context", json!({})).await["structuredContent"]
            ["operations"],
        json!([])
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_flag_without_runtime_cannot_admit_or_create_permanent_refusal() {
    let c = ControlFixture::with_callback_route().await;
    let value = body(&c, "http-production-unavailable", 502, false);
    let before_dispositions = rows(&c, "bridge_callback_dispositions").await;
    let runtime = Arc::new(Runtime {
        registry: Arc::new(crate::sandbox::LiveAgentRegistry::default()),
        calls: AtomicUsize::new(0),
    });
    let (status, result) = post(app(&c, true, Some(runtime)).await, &c, &value).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    assert_eq!(result["acceptance"], "unknown");
    no_message_effects(&c, 0).await;
    assert_eq!(
        rows(&c, "bridge_callback_dispositions").await,
        before_dispositions
    );
    let (status, result) = post(app(&c, true, None).await, &c, &value).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{result}");
    assert_eq!(result["status"], "control_accepted");
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_requires_exact_sender_chat_thread_and_confirmed_source_reply() {
    let c = ControlFixture::with_callback_route().await;
    let runtime = Runtime::local(&c);
    for (index, kind) in ["sender", "chat", "thread", "parent", "reference"]
        .into_iter()
        .enumerate()
    {
        let mut value = body(&c, &format!("http-wrong-{kind}"), 510 + index as u64, false);
        match kind {
            "sender" => {
                value["sender"]["platform_id"] = json!("777");
                value["raw_platform_data"]["message"]["from"]["id"] = json!(777);
            }
            "chat" => {
                value["conversation"]["platform_id"] = json!("-100888");
                value["raw_platform_data"]["message"]["chat"]["id"] = json!(-100888);
                value["raw_platform_data"]["message"]["reply_to_message"]["chat"]["id"] =
                    json!(-100888);
            }
            "thread" => {
                value["raw_platform_data"]["message"]["message_thread_id"] = json!(99);
                value["raw_platform_data"]["message"]["is_topic_message"] = json!(true);
            }
            "parent" => {
                value
                    .as_object_mut()
                    .unwrap()
                    .remove("reply_to_platform_message_id");
                value["raw_platform_data"]["message"]
                    .as_object_mut()
                    .unwrap()
                    .remove("reply_to_message");
            }
            _ => {
                let text = "CMA-CONTROL/1 cancel ccr_00000000000000000000000000000000";
                value["content"]["text"] = json!(text);
                value["raw_platform_data"]["message"]["text"] = json!(text);
            }
        }
        let (status, result) = post(app(&c, true, Some(runtime.clone())).await, &c, &value).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{kind}: {result}");
        no_message_effects(&c, 0).await;
    }
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_missing_local_runtime_preserves_retry_without_ordinary_input() {
    let c = ControlFixture::with_callback_route().await;
    let runtime = Arc::new(Runtime {
        registry: Arc::new(crate::sandbox::LiveAgentRegistry::default()),
        calls: AtomicUsize::new(0),
    });
    let value = body(&c, "http-missing-runtime", 520, false);
    let (status, result) = post(app(&c, true, Some(runtime.clone())).await, &c, &value).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{result}");
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
    no_message_effects(&c, 0).await;
    assert_eq!(rows(&c, "bridge_refused_control_intents").await, 0);
    let (status, result) = post(app(&c, true, Some(Runtime::local(&c))).await, &c, &value).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{result}");
    no_message_effects(&c, 1).await;
    c.f.close().await;
}
