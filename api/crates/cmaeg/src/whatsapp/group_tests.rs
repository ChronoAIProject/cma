use super::*;
use crate::{
    net::exchange::{HttpReply, TransportError},
    whatsapp::{
        tests::{credentials, envelope, source, text},
        webhook, Transport,
    },
};
use cmaeg_core::{
    channel::whatsapp::WhatsappGroupReply,
    message::{OutboundMessage, WhatsappTemplateComponent},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

pub(crate) const GROUP: &str = "group_A+/==";
pub(crate) fn source_group() -> Source {
    let mut value = source();
    if let Source::Whatsapp { group_ids, .. } = &mut value {
        *group_ids = vec![GROUP.into()];
    }
    value
}
pub(crate) fn reply() -> WhatsappGroupReply {
    WhatsappGroupReply {
        revision: crate::whatsapp::group_revision(&source_group(), &credentials()),
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
        group_id: GROUP.into(),
        participant_id: "447700900123".into(),
        message_id: "wamid.original".into(),
    }
}
pub(crate) fn group_text(id: &str) -> Value {
    let mut value = text(id, "hello group");
    value["group_id"] = GROUP.into();
    value
}
pub(crate) fn template() -> OutboundMessage {
    let mut message = crate::whatsapp::templates::tests::rich();
    message
        .whatsapp_template
        .as_mut()
        .unwrap()
        .components
        .retain(|component| !matches!(component, WhatsappTemplateComponent::Button { .. }));
    message
}
pub(crate) fn answer(value: Value) -> Result<HttpReply, TransportError> {
    Ok(HttpReply::new(
        StatusCode::OK,
        serde_json::to_vec(&value).unwrap(),
    ))
}
pub(crate) fn proof_values() -> Vec<Value> {
    vec![
        json!({"id":"34567","official_business_account":{"oba_status":"APPROVED"}}),
        json!({"data":{"groups":[{"id":GROUP}]}}),
        json!({"id":GROUP,"messaging_product":"whatsapp","suspended":false,"total_participant_count":1,"participants":[{"wa_id":"447700900123"}]}),
    ]
}
pub(crate) fn proofs() -> Vec<Result<HttpReply, TransportError>> {
    proof_values().into_iter().map(answer).collect()
}
pub(crate) fn confirmed(id: &str) -> Result<HttpReply, TransportError> {
    answer(json!({"messaging_product":"whatsapp","messages":[{"id":id,"group_id":GROUP}]}))
}
pub(crate) type Call = (Method, Url, Vec<u8>);
pub(crate) struct Graph {
    pub calls: Mutex<Vec<Call>>,
    replies: Mutex<VecDeque<Result<HttpReply, TransportError>>>,
}
impl Graph {
    pub(crate) fn new(replies: Vec<Result<HttpReply, TransportError>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            replies: Mutex::new(replies.into()),
        })
    }
}
impl Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert!(matches!(method, Method::GET | Method::POST));
        assert_eq!(url.host_str(), Some("graph.facebook.com"));
        assert!(headers["authorization"].is_sensitive());
        self.calls.lock().unwrap().push((method, url, body));
        let result = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected Graph call");
        Box::pin(async move { result })
    }
}
#[tokio::test]
async fn native_whatsapp_groups_prove_exact_phone_oba_active_membership_and_original_participant() {
    for defect in 0..10 {
        let mut values = proof_values();
        match defect {
            0 => values[0]["id"] = "99999".into(),
            1 => values[0]["official_business_account"]["oba_status"] = "PENDING".into(),
            2 => values[1]["data"]["groups"] = json!([]),
            3 => values[2]["id"] = "other".into(),
            4 => values[2]["suspended"] = true.into(),
            5 => values[2]["suspended"] = "false".into(),
            6 => values[2]["participants"][0]["wa_id"] = "447700900999".into(),
            7 => values[2]["total_participant_count"] = 2.into(),
            8 => values[2]["participants"] = json!([]),
            9 => {}
            _ => unreachable!(),
        }
        let graph = Graph::new(values.into_iter().map(answer).collect());
        let client = Client::production().unwrap().with_transport(graph.clone());
        let result = client
            .verify_groups(
                &credentials(),
                "34567",
                &[GROUP.into()],
                Some("447700900123"),
            )
            .await;
        assert_eq!(
            result,
            if defect == 9 {
                Ok(())
            } else {
                Err(Error::CustodyUnproved)
            },
            "{defect}"
        );
        let calls = graph.calls.lock().unwrap();
        assert!(calls
            .iter()
            .all(|(method, _, body)| *method == Method::GET && body.is_empty()));
        assert_eq!(
            calls[0].1.as_str(),
            "https://graph.facebook.com/v25.0/34567?fields=official_business_account"
        );
        if calls.len() == 3 {
            assert!(calls[2].1.path().contains("group_A+%2F=="));
        }
    }
}
#[tokio::test]
async fn native_whatsapp_groups_ignore_provider_next_urls_and_bound_pagination() {
    let mut values = proof_values();
    values.insert(1, json!({"data":{"groups":[]},"paging":{"next":"https://evil.invalid/steal","cursors":{"after":"opaque+/="}}}));
    let graph = Graph::new(values.into_iter().map(answer).collect());
    let client = Client::production().unwrap().with_transport(graph.clone());
    assert_eq!(
        client
            .verify_groups(&credentials(), "34567", &[GROUP.into()], None)
            .await,
        Ok(())
    );
    {
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls[2].1.path(), "/v25.0/34567/groups");
        assert_eq!(
            calls[2]
                .1
                .query_pairs()
                .find(|(key, _)| key == "after")
                .unwrap()
                .1,
            "opaque+/="
        );
    }
    let mut values = vec![proof_values().remove(0)];
    values.extend((0..4).map(|i| json!({"data":{"groups":[]},"paging":{"next":"https://evil.invalid", "cursors":{"after":format!("cursor{i}")}}})));
    let graph = Graph::new(values.into_iter().map(answer).collect());
    assert_eq!(
        Client::production()
            .unwrap()
            .with_transport(graph.clone())
            .verify_groups(&credentials(), "34567", &[GROUP.into()], None)
            .await,
        Err(Error::CustodyUnproved)
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 5);
}
#[test]
fn native_whatsapp_group_parser_retains_participant_and_rejects_unconfigured_or_unsupported_authority(
) {
    let values = webhook::extract(
        &source_group(),
        &envelope(vec![group_text("wamid.original")]),
    )
    .unwrap();
    assert_eq!(values[0].group_id.as_deref(), Some(GROUP));
    assert_eq!(values[0].sender, "447700900123");
    assert_eq!(values[0].conversation(), GROUP);
    assert!(webhook::extract(&source(), &envelope(vec![group_text("wamid.original")])).is_err());
    let mut reaction = crate::whatsapp::tests::reaction("wamid.reaction", "wamid.original", "👍");
    reaction["group_id"] = GROUP.into();
    assert!(webhook::extract(&source_group(), &envelope(vec![reaction]))
        .unwrap()
        .is_empty());
    let mut changed = group_text("wamid.original");
    changed["from"] = "447700900999".into();
    assert!(webhook::extract(
        &source_group(),
        &envelope(vec![group_text("wamid.original"), changed])
    )
    .is_err());
}
#[tokio::test]
async fn native_whatsapp_group_sends_require_exact_group_confirmation_and_never_retry() {
    for response in [
        json!({}),
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":GROUP}],"messages":[{"id":"wamid.sent"}]}),
        json!({"messaging_product":"whatsapp","messages":[{"id":"wamid.sent","group_id":"other"}]}),
    ] {
        let graph = Graph::new(vec![answer(response)]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        assert_eq!(
            client
                .send_group_text(&credentials(), &reply(), "hello")
                .await,
            Err(Error::Unconfirmed)
        );
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let body: Value = serde_json::from_slice(&calls[0].2).unwrap();
        assert_eq!(body["recipient_type"], "group");
        assert_eq!(body["to"], GROUP);
        assert_eq!(body["context"]["message_id"], "wamid.original");
    }
}

struct Pending;
impl Transport for Pending {
    fn exchange(
        &self,
        _: Method,
        _: Url,
        _: HeaderMap,
        _: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test(start_paused = true)]
async fn native_whatsapp_group_custody_has_a_total_read_budget() {
    let client = Client::production()
        .unwrap()
        .with_transport(Arc::new(Pending));
    let start = tokio::time::Instant::now();
    assert_eq!(
        client
            .verify_groups(&credentials(), "34567", &[GROUP.into()], None)
            .await,
        Err(Error::Unconfirmed)
    );
    assert_eq!(start.elapsed(), Duration::from_secs(12));
}
