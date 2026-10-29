use super::{
    outbound::tests::{answer, confirmed, Graph},
    structured::PreparedStructured,
    tests::credentials,
    Client, Error,
};
use crate::net::exchange::{HttpReply, TransportError};
use cmaeg_core::message::StructuredContent;
use reqwest::StatusCode;
use serde_json::{json, Value};

pub(crate) fn samples() -> Vec<(Value, Value)> {
    vec![
        (
            json!({"type":"contact","phone_number":"+6512345678","first_name":"Alice","last_name":"Example"}),
            json!({"name":{"formatted_name":"Alice Example","first_name":"Alice","last_name":"Example"},"phones":[{"phone":"+6512345678"}]}),
        ),
        (
            json!({"type":"location","latitude":1.3521,"longitude":103.8198}),
            json!({"latitude":"1.3521","longitude":"103.8198"}),
        ),
        (
            json!({"type":"venue","latitude":1.3521,"longitude":103.8198,"title":"Meeting point","address":"Singapore"}),
            json!({"latitude":"1.3521","longitude":"103.8198","name":"Meeting point","address":"Singapore"}),
        ),
    ]
}

#[tokio::test]
async fn official_payloads_use_original_private_phone_recipient_and_parent_only() {
    for (content, expected) in samples() {
        let graph = Graph::new(vec![confirmed("wamid.structured")]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        let typed: StructuredContent = serde_json::from_value(content.clone()).unwrap();
        let prepared =
            PreparedStructured::new("34567", "447700900123", "wamid.original", &typed).unwrap();
        assert_eq!(
            client.send_structured(&credentials(), prepared).await,
            Ok("wamid.structured".into())
        );
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.path(), "/v25.0/34567/messages");
        let kind = if content["type"] == "contact" {
            "contacts"
        } else {
            "location"
        };
        let mut body = json!({"messaging_product":"whatsapp","recipient_type":"individual","to":"447700900123", "context":{"message_id":"wamid.original"}, "type":kind});
        body[kind] = if kind == "contacts" {
            json!([expected])
        } else {
            expected
        };
        assert_eq!(serde_json::from_slice::<Value>(&calls[0].2).unwrap(), body);
        assert!(!String::from_utf8_lossy(&calls[0].2).contains(credentials().access_token.as_str()));
    }
}

#[test]
fn unsupported_fields_and_invalid_destinations_are_never_silently_dropped() {
    for value in [
        json!({"type":"contact","phone_number":"123","first_name":"Alice","vcard":""}),
        json!({"type":"location","latitude":1,"longitude":2,"horizontal_accuracy":0}),
        json!({"type":"location","latitude":91,"longitude":2}),
        json!({"type":"venue","latitude":1,"longitude":2,"title":"","address":"Street"}),
    ] {
        let typed = serde_json::from_value(value).unwrap();
        assert!(
            PreparedStructured::new("34567", "447700900123", "wamid.original", &typed).is_err()
        );
    }
    let typed = serde_json::from_value(samples()[0].0.clone()).unwrap();
    for (phone, recipient, parent) in [
        ("../phone", "447700900123", "wamid.original"),
        ("34567", "group@example", "wamid.original"),
        ("34567", "447700900123", ""),
    ] {
        assert!(PreparedStructured::new(phone, recipient, parent, &typed).is_err());
    }
}

#[tokio::test]
async fn wrong_recipient_missing_or_multiple_receipts_and_transport_uncertainty_never_retry() {
    for case in 0..6 {
        let mut value = json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.confirmed"}]});
        match case {
            0 => value["contacts"][0]["wa_id"] = "447700900999".into(),
            1 => value["messages"] = json!([]),
            2 => value["messages"] = json!([{"id":"wamid.one"},{"id":"wamid.two"}]),
            3 => value["messaging_product"] = "other".into(),
            _ => {}
        }
        let reply = match case {
            4 => Err(TransportError::Unavailable),
            5 => Ok(HttpReply::new(
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":130429}}"#.to_vec(),
            )),
            _ => answer(value),
        };
        let graph = Graph::new(vec![reply]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        let prepared = PreparedStructured::new(
            "34567",
            "447700900123",
            "wamid.original",
            &serde_json::from_value(samples()[1].0.clone()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            client.send_structured(&credentials(), prepared).await,
            Err(if case == 5 {
                Error::RateLimited
            } else {
                Error::Unconfirmed
            })
        );
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
}
