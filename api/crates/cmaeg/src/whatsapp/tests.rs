use super::{
    webhook::{self, Content},
    Client, Error, Transport,
};
use crate::net::exchange::{HttpReply, TransportError};
use cmaeg_core::channel::{AdmissionPolicy, Secret, Source, WhatsappCredentials};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) fn source() -> Source {
    Source::Whatsapp {
        group_ids: vec![],
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
        admission: AdmissionPolicy::All {},
    }
}
pub(crate) fn credentials() -> WhatsappCredentials {
    WhatsappCredentials {
        app_secret: Secret::new("fixture-app-secret-12345"),
        access_token: Secret::new("fixture-access-token-12345"),
        verify_token: Secret::new("fixture-verify-token-12345"),
    }
}
pub(crate) fn envelope(messages: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"object":"whatsapp_business_account","entry":[{"id":"23456","changes":[{"field":"messages","value":{"messaging_product":"whatsapp","metadata":{"phone_number_id":"34567"},"messages":messages}}]}]})).unwrap()
}
pub(crate) fn text(id: &str, body: &str) -> Value {
    json!({"from":"447700900123","id":id,"timestamp":"1800000000","type":"text","text":{"body":body}})
}
pub(crate) fn reaction(id: &str, target: &str, emoji: &str) -> Value {
    json!({"from":"447700900123","id":id,"timestamp":"1800000001","type":"reaction","reaction":{"message_id":target,"emoji":emoji}})
}

#[test]
fn native_whatsapp_signature_binds_original_bytes_and_exact_account_phone() {
    let bytes = envelope(vec![text("wamid.original", "hello")]);
    let secret = credentials();
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(
            ring::hmac::HMAC_SHA256,
            secret.app_secret.as_str().as_bytes(),
        ),
        &bytes,
    );
    let signature = format!("sha256={}", webhook::hex(tag.as_ref()));
    assert_eq!(
        webhook::verify(secret.app_secret.as_str(), &signature, &bytes),
        Ok(())
    );
    let mut changed = bytes.clone();
    changed.push(b' ');
    assert!(webhook::verify(secret.app_secret.as_str(), &signature, &changed).is_err());
    assert!(webhook::verify("another-app-secret", &signature, &bytes).is_err());
    let mut other = source();
    if let Source::Whatsapp { waba_id, .. } = &mut other {
        *waba_id = "99999".into();
    }
    assert!(webhook::extract(&other, &bytes).unwrap().is_empty());
    if let Source::Whatsapp {
        waba_id,
        phone_number_id,
        ..
    } = &mut other
    {
        *waba_id = "23456".into();
        *phone_number_id = "99999".into();
    }
    assert!(webhook::extract(&other, &bytes).unwrap().is_empty());
    assert_eq!(webhook::extract(&source(), &bytes).unwrap().len(), 1);
}

#[test]
fn native_whatsapp_preserves_replacement_removal_actor_and_message_identity() {
    let messages = webhook::extract(
        &source(),
        &envelope(vec![
            reaction("wamid.reaction-one", "wamid.original", "👍"),
            reaction("wamid.reaction-two", "wamid.original", "❤️"),
            reaction("wamid.reaction-three", "wamid.original", ""),
        ]),
    )
    .unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].sender, "447700900123");
    assert!(
        matches!(&messages[2].content, Content::Reaction { target, emoji } if target == "wamid.original" && emoji.is_empty())
    );
    assert_ne!(
        webhook::event_id(&source(), &messages[0].id).unwrap(),
        webhook::event_id(&source(), &messages[1].id).unwrap()
    );
    let mut removed = reaction("wamid.reaction-four", "wamid.original", "");
    removed["reaction"].as_object_mut().unwrap().remove("emoji");
    let removed = webhook::extract(&source(), &envelope(vec![removed])).unwrap();
    assert!(
        matches!(&removed[0].content, Content::Reaction { target, emoji }
        if target == "wamid.original" && emoji.is_empty())
    );
}

#[test]
fn native_whatsapp_malformed_reactions_never_become_removals() {
    for emoji in [
        Value::Null,
        json!(false),
        json!(0),
        json!([]),
        json!({}),
        json!("\n"),
        json!("a".repeat(513)),
    ] {
        let mut malformed = reaction("wamid.malformed", "wamid.original", "");
        malformed["reaction"]["emoji"] = emoji;
        assert_eq!(
            webhook::extract(&source(), &envelope(vec![malformed])),
            Err(webhook::Error::Message)
        );
    }
    for field in ["reaction", "from", "timestamp", "id"] {
        let mut malformed = reaction("wamid.malformed", "wamid.original", "");
        malformed.as_object_mut().unwrap().remove(field);
        assert!(webhook::extract(&source(), &envelope(vec![malformed])).is_err());
    }
    let mut malformed = reaction("wamid.malformed", "wamid.original", "");
    malformed["reaction"].as_object_mut().unwrap().clear();
    assert!(webhook::extract(&source(), &envelope(vec![malformed])).is_err());
    let mut malformed = text("wamid.malformed", "hello");
    malformed["text"].as_object_mut().unwrap().clear();
    assert!(webhook::extract(&source(), &envelope(vec![malformed])).is_err());
}

#[test]
fn native_whatsapp_conflicting_duplicate_ids_in_one_signed_envelope_are_refused() {
    let same = text("wamid.same", "same");
    assert_eq!(
        webhook::extract(&source(), &envelope(vec![same.clone(), same.clone()]))
            .unwrap()
            .len(),
        1
    );
    assert!(webhook::extract(
        &source(),
        &envelope(vec![same, text("wamid.same", "changed")])
    )
    .is_err());
}

#[test]
fn native_whatsapp_group_facts_cannot_be_projected_as_a_private_sender_chat() {
    for field in ["group_id", "group", "participant", "recipient_type"] {
        let mut message = text("wamid.group", "hello");
        message[field] = "group".into();
        assert!(webhook::extract(&source(), &envelope(vec![message])).is_err());
    }
}

type Calls = Vec<(Method, Url, HeaderMap, Vec<u8>)>;
struct Fixture {
    answers: Mutex<VecDeque<(StatusCode, Value)>>,
    calls: Mutex<Calls>,
}
impl Transport for Fixture {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls
            .lock()
            .unwrap()
            .push((method, url, headers, body));
        let (status, body) = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected Graph call");
        Box::pin(async move {
            Ok(HttpReply {
                status,
                headers: HeaderMap::new(),
                body: Ok(serde_json::to_vec(&body).unwrap()),
            })
        })
    }
}
fn fixture_client(answers: Vec<Value>) -> (Client, Arc<Fixture>) {
    let fixture = Arc::new(Fixture {
        answers: Mutex::new(
            answers
                .into_iter()
                .map(|value| (StatusCode::OK, value))
                .collect(),
        ),
        calls: Mutex::default(),
    });
    (
        Client::production()
            .unwrap()
            .with_transport(fixture.clone()),
        fixture,
    )
}
fn valid_token() -> Value {
    json!({"data":{"is_valid":true,"app_id":"12345","scopes":["whatsapp_business_management","whatsapp_business_messaging"]}})
}

#[tokio::test]
async fn native_whatsapp_custody_requires_graph_app_scope_and_waba_phone_proof() {
    for defect in 0..4 {
        let mut token = valid_token();
        if defect == 0 {
            token["data"]["app_id"] = "99999".into();
        }
        if defect == 1 {
            token["data"]["is_valid"] = false.into();
        }
        if defect == 2 {
            token["data"]["scopes"] = json!(["whatsapp_business_messaging"]);
        }
        let (client, fixture) = fixture_client(vec![
            token,
            json!({"data":[{"id":if defect == 3 { "99999" } else { "34567" }}]}),
        ]);
        assert_eq!(
            client.verify_custody(&source(), &credentials()).await,
            Err(Error::CustodyUnproved)
        );
        assert_eq!(
            fixture.calls.lock().unwrap().len(),
            if defect == 3 { 2 } else { 1 }
        );
    }
    let (client, fixture) = fixture_client(vec![valid_token(), json!({"data":[{"id":"34567"}]})]);
    assert_eq!(
        client.verify_custody(&source(), &credentials()).await,
        Ok(())
    );
    let calls = fixture.calls.lock().unwrap();
    assert!(calls
        .iter()
        .all(|(method, url, _, _)| *method == Method::GET
            && url.host_str() == Some("graph.facebook.com")));
    assert_eq!(calls[0].1.path(), "/v25.0/debug_token");
    assert!(calls[0].2["authorization"].is_sensitive());
    assert_eq!(calls[1].1.path(), "/v25.0/23456/phone_numbers");
}

#[tokio::test]
async fn native_whatsapp_replies_are_real_phone_sends_and_unknown_receipts_are_not_retried() {
    let (client, fixture) = fixture_client(vec![
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.sent"}]}),
    ]);
    assert_eq!(
        client
            .send_text(
                &credentials(),
                "34567",
                "447700900123",
                "wamid.original",
                "reply"
            )
            .await,
        Ok("wamid.sent".into())
    );
    {
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Method::POST);
        assert_eq!(calls[0].1.path(), "/v25.0/34567/messages");
        let body: Value = serde_json::from_slice(&calls[0].3).unwrap();
        assert_eq!(body["to"], "447700900123");
        assert_eq!(body["context"]["message_id"], "wamid.original");
    }
    for invalid in [
        json!({}),
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"99999"}],"messages":[{"id":"wamid.sent"}]}),
    ] {
        let (client, fixture) = fixture_client(vec![invalid]);
        assert_eq!(
            client
                .send_text(
                    &credentials(),
                    "34567",
                    "447700900123",
                    "wamid.original",
                    "reply"
                )
                .await,
            Err(Error::Unconfirmed)
        );
        assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    }
}
