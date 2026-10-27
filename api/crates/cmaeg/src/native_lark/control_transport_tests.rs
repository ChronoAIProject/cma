use super::{test_support::identity, Client, Error, Transport};
use crate::net::exchange::{HttpReply, TransportError};
use cmaeg_core::channel::{LarkRegion, NativeLarkReply, Secret};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
struct Api {
    calls: Mutex<Vec<(Method, Url, Value)>>,
    response: Value,
}
impl Transport for Api {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls.lock().unwrap().push((
            method,
            url,
            if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            },
        ));
        Box::pin(async {
            Ok(HttpReply::new(
                StatusCode::OK,
                serde_json::to_vec(&self.response).unwrap(),
            ))
        })
    }
}
fn client(response: Value) -> (Client, Arc<Api>) {
    let io = Arc::new(Api {
        calls: Mutex::default(),
        response,
    });
    (Client::production().unwrap().with_transport(io.clone()), io)
}
fn reply(region: LarkRegion, thread: bool) -> NativeLarkReply {
    NativeLarkReply {
        identity: cmaeg_core::channel::NativeLarkIdentity {
            region,
            ..identity()
        },
        chat_id: "oc_native_chat".into(),
        message_id: "om_original".into(),
        thread_id: thread.then(|| "omt_original".into()),
        revision: "0".repeat(64),
        mentioned: false,
    }
}
fn card() -> Value {
    json!({"schema":"2.0","config":{"update_multi":true},"body":{"elements":[]}})
}
#[tokio::test]
async fn direct_cards_use_regional_create_or_exact_thread_reply_and_patch_string_content() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for thread in [false, true] {
            let reply = reply(region, thread);
            let token = Secret::new("fixture-control-access-token");
            let mut data = json!({"message_id":"om_sent","chat_id":reply.chat_id});
            if thread {
                data["thread_id"] = "omt_original".into();
            }
            let (client, io) = client(json!({"code":0,"data":data}));
            assert_eq!(
                client.send_card(&token, &reply, &card()).await.unwrap(),
                "om_sent"
            );
            let calls = io.calls.lock().unwrap();
            let (method, url, body) = &calls[0];
            assert_eq!(*method, Method::POST);
            assert_eq!(url.origin().ascii_serialization(), region.origin());
            assert_eq!(
                url.path(),
                if thread {
                    "/open-apis/im/v1/messages/om_original/reply"
                } else {
                    "/open-apis/im/v1/messages"
                }
            );
            assert_eq!(body["msg_type"], "interactive");
            assert_eq!(
                serde_json::from_str::<Value>(body["content"].as_str().unwrap()).unwrap(),
                card()
            );
            if thread {
                assert_eq!(body["reply_in_thread"], true);
            }
            drop(calls);
            let (client, io) = self::client(json!({"code":0,"data":{}}));
            client.patch_card(&token, &reply, &card()).await.unwrap();
            let calls = io.calls.lock().unwrap();
            assert_eq!(calls[0].0, Method::PATCH);
            assert_eq!(calls[0].1.path(), "/open-apis/im/v1/messages/om_original");
            assert_eq!(calls[0].2.as_object().unwrap().len(), 1);
        }
    }
}
#[tokio::test]
async fn card_metadata_and_malformed_patch_success_never_invent_authority() {
    let reply = reply(LarkRegion::Feishu, true);
    let token = Secret::new("fixture-control-access-token");
    let row = json!({"message_id":reply.message_id,"chat_id":reply.chat_id,"thread_id":"omt_original","msg_type":"interactive","deleted":false,"sender":{"sender_type":"app","id_type":"app_id","id":reply.identity.app_id,"tenant_key":reply.identity.tenant_key}});
    for case in 0..6 {
        let mut row = row.clone();
        match case {
            1 => row["thread_id"] = "omt_foreign".into(),
            2 => row["sender"]["id"] = "cli_foreign".into(),
            3 => row["deleted"] = true.into(),
            4 => row["msg_type"] = "text".into(),
            5 => row["chat_id"] = "oc_foreign".into(),
            _ => {}
        }
        let (client, _) = client(json!({"code":0,"data":{"items":[row]}}));
        assert_eq!(client.card_message(&token, &reply).await.is_ok(), case == 0);
    }
    for data in [json!(null), json!({"message_id":"om_new"}), json!([])] {
        let (client, _) = client(json!({"code":0,"data":data}));
        assert!(matches!(
            client.patch_card(&token, &reply, &card()).await,
            Err(Error::Unconfirmed)
        ));
    }
    let (client, io) = client(json!({"code":0,"data":{}}));
    let mut oversized = card();
    oversized["body"]["text"] = "x".repeat(28 * 1024).into();
    assert!(client.patch_card(&token, &reply, &oversized).await.is_err());
    assert!(io.calls.lock().unwrap().is_empty());
}
