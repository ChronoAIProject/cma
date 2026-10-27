use super::{
    test_support::{credentials, identity},
    *,
};
use crate::net::exchange::{HttpReply, TransportError};
use cmaeg_core::channel::{LarkRegion, NativeLarkReply, Secret};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
struct Fake {
    calls: Mutex<Vec<(Method, Url, HeaderMap, Value)>>,
    bot: String,
    response: Value,
    status: StatusCode,
}
impl Transport for Fake {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        let result = if url.path() == "/open-apis/auth/v3/tenant_access_token/internal" {
            json!({"code":0,"tenant_access_token":"fixture-native-access-token","expire":7200})
        } else if url.path() == "/open-apis/bot/v3/info" {
            json!({"code":0,"bot":{"open_id":self.bot}})
        } else {
            self.response.clone()
        };
        self.calls.lock().unwrap().push((
            method,
            url,
            headers,
            if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            },
        ));
        Box::pin(async move {
            Ok(HttpReply::new(
                self.status,
                serde_json::to_vec(&result).unwrap(),
            ))
        })
    }
}
fn fake(response: Value) -> Arc<Fake> {
    Arc::new(Fake {
        calls: Mutex::default(),
        bot: "ou_native_bot".into(),
        response,
        status: StatusCode::OK,
    })
}
fn reply() -> NativeLarkReply {
    NativeLarkReply {
        identity: identity(),
        chat_id: "oc_native_chat".into(),
        message_id: "om_original".into(),
        thread_id: None,
        revision: "0".repeat(64),
        mentioned: false,
    }
}
#[tokio::test]
async fn regional_custody_uses_only_pinned_custom_app_token_and_independent_bot_identity() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        let io = fake(Value::Null);
        let client = Client::production().unwrap().with_transport(io.clone());
        let mut identity = identity();
        identity.region = region;
        client
            .verify_custody(&identity, &credentials())
            .await
            .unwrap();
        let calls = io.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (_, url, _, _) in calls.iter() {
            assert_eq!(url.origin().ascii_serialization(), region.origin());
        }
        assert_eq!(calls[0].0, Method::POST);
        assert_eq!(calls[0].3["app_id"], identity.app_id);
        assert_eq!(calls[0].3["app_secret"], credentials().app_secret.as_str());
        assert!(!calls[0].2.contains_key("authorization"));
        assert_eq!(calls[1].0, Method::GET);
        assert_eq!(
            calls[1].2["authorization"],
            "Bearer fixture-native-access-token"
        );
        assert!(calls[1].3.is_null());
    }
    let mut io = fake(Value::Null);
    Arc::get_mut(&mut io).unwrap().bot = "ou_other_bot".into();
    assert!(Client::production()
        .unwrap()
        .with_transport(io)
        .verify_custody(&identity(), &credentials())
        .await
        .is_err());
}
#[tokio::test]
async fn native_text_receipts_require_exact_chat_thread_and_new_message_without_retry() {
    let good = json!({"code":0,"data":{"message_id":"om_sent","chat_id":"oc_native_chat"}});
    for mutation in [
        "good",
        "wrong_chat",
        "same_message",
        "missing_message",
        "wrong_thread",
        "api_error",
    ] {
        let mut response = good.clone();
        match mutation {
            "wrong_chat" => response["data"]["chat_id"] = "oc_other".into(),
            "same_message" => response["data"]["message_id"] = "om_original".into(),
            "missing_message" => response["data"]["message_id"] = Value::Null,
            "wrong_thread" => response["data"]["thread_id"] = "omt_other".into(),
            "api_error" => response["code"] = 999.into(),
            _ => {}
        }
        let io = fake(response);
        let client = Client::production().unwrap().with_transport(io.clone());
        let result = client
            .send_text(&Secret::new("fixture-token"), &reply(), "hello")
            .await;
        assert_eq!(result.is_ok(), mutation == "good");
        let calls = io.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].3["receive_id"], "oc_native_chat");
        assert_eq!(calls[0].3["msg_type"], "text");
        assert_eq!(calls[0].3["content"], json!({"text":"hello"}).to_string());
    }
    let mut destination = reply();
    destination.thread_id = Some("omt_thread".into());
    let io = fake(
        json!({"code":0,"data":{"message_id":"om_sent","chat_id":"oc_native_chat","thread_id":"omt_thread"}}),
    );
    Client::production()
        .unwrap()
        .with_transport(io.clone())
        .send_text(&Secret::new("fixture-token"), &destination, "thread reply")
        .await
        .unwrap();
    let calls = io.calls.lock().unwrap();
    assert_eq!(
        calls[0].1.path(),
        "/open-apis/im/v1/messages/om_original/reply"
    );
    assert_eq!(calls[0].3["reply_in_thread"], true);
    assert!(calls[0].3.get("receive_id").is_none());
}
#[tokio::test]
async fn redirect_rate_limit_and_malformed_success_never_trigger_a_second_write() {
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::OK,
    ] {
        let mut io = fake(json!({"unexpected":"shape"}));
        Arc::get_mut(&mut io).unwrap().status = status;
        let result = Client::production()
            .unwrap()
            .with_transport(io.clone())
            .send_text(&Secret::new("fixture-token"), &reply(), "hello")
            .await;
        assert!(result.is_err());
        assert_eq!(io.calls.lock().unwrap().len(), 1);
    }
}
