use super::PreparedTemplate;
use crate::{
    net::exchange::{HttpReply, TransportError},
    whatsapp::{tests::credentials, Client, Transport},
};
use cmaeg_core::message::OutboundMessage;
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) fn message(header: Value) -> OutboundMessage {
    serde_json::from_value(json!({"whatsapp_template": {
        "name":"invoice_ready", "language":{"code":"en_US"}, "components":[
            {"type":"header","parameters":[header]},
            {"type":"body","parameters":[
                {"type":"text","text":"Order 42"},
                {"type":"currency","currency":{"fallback_value":"USD 10","code":"USD","amount_1000":10000}},
                {"type":"date_time","date_time":{"fallback_value":"Tomorrow"}}
            ]},
            {"type":"button","sub_type":"url","index":"0","parameters":[{"type":"text","text":"order/42"}]},
            {"type":"button","sub_type":"url","index":"1","parameters":[{"type":"text","text":"invoice/42"}]},
            {"type":"button","sub_type":"url","index":"2","parameters":[{"type":"text","text":"support/42"}]}
        ]
    }})).unwrap()
}
pub(crate) fn rich() -> OutboundMessage {
    message(json!({"type":"document","document":{"id":"123456789","filename":"invoice.pdf"}}))
}

struct Budget(Mutex<Vec<(Duration, Value)>>);
impl Transport for Budget {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, Method::POST);
        assert_eq!(
            url.as_str(),
            "https://graph.facebook.com/v25.0/34567/messages"
        );
        assert_eq!(
            headers["authorization"],
            format!("Bearer {}", credentials().access_token.as_str())
        );
        self.0
            .lock()
            .unwrap()
            .push((timeout, serde_json::from_slice(&body).unwrap()));
        Box::pin(async {
            Ok(HttpReply::new(StatusCode::OK,serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.template"}]})).unwrap()))
        })
    }
}
#[tokio::test]
async fn native_whatsapp_templates_preserve_full_typed_contract_and_media_budget() {
    let mut headers = vec![json!({"type":"text","text":"Invoice ready"})];
    for kind in ["image", "video", "document"] {
        for reference in [
            json!({"id":"123456789"}),
            json!({"link":"https://media.example/approved"}),
        ] {
            headers.push(json!({"type":kind,kind:reference}));
        }
    }
    headers.push(json!({"type":"document","document":{"id":"123456789","filename":"invoice.pdf"}}));
    let mut messages: Vec<_> = headers.into_iter().map(message).collect();
    messages.push(
        serde_json::from_value(
            json!({"whatsapp_template":{"name":"parameterless","language":{"code":"en"}}}),
        )
        .unwrap(),
    );
    messages.push(serde_json::from_value(json!({"whatsapp_template":{"name":"maximum_body","language":{"code":"en_US"},"components":[{"type":"body","parameters":vec![json!({"type":"text","text":"\"".repeat(1024)});20]}]}})).unwrap());
    for message in messages {
        let template = message.whatsapp_template.as_ref().unwrap();
        let graph = Arc::new(Budget(Mutex::default()));
        let client = Client::production().unwrap().with_transport(graph.clone());
        let prepared =
            PreparedTemplate::new("34567", "447700900123", "wamid.original", template).unwrap();
        assert_eq!(
            client.send_template(&credentials(), prepared).await,
            Ok("wamid.template".into())
        );
        let calls = graph.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].0,
            Duration::from_secs(if template.has_media() { 60 } else { 8 })
        );
        assert_eq!(
            calls[0].1,
            json!({"messaging_product":"whatsapp","recipient_type":"individual","to":"447700900123","context":{"message_id":"wamid.original"},"type":"template","template":template})
        );
    }
}
