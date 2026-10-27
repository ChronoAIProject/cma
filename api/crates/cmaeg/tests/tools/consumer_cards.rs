//! Cards a consumer prepares are that consumer's: read through its binding,
//! sealed as `ic`, referenced for it, and clicked only when the admission
//! gate admits it; a default card keeps its exact bytes and `in` kind (I3).
use super::{
    consumer_support::{issue, reviewer, reviewer_key, routed, REVIEWER_BINDING},
    fake_cma::FakeCma,
    relay_support::now,
    support::App,
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    interactions::{Destination, Error, InteractionStore, PreparedCard},
    records::RecordClient,
};
use cmaeg_core::{
    ids::{ConsumerId, MessageRef},
    message::OutboundMessage,
    seal::{open_consumer_interaction, open_interaction, EventRef, MessageRefPayload},
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

struct Cards {
    app: App,
    backend: FakeCma,
    store: InteractionStore,
}

impl Cards {
    async fn new() -> Self {
        let app = routed().await;
        let backend = FakeCma::start(false).await;
        let store = InteractionStore::new(
            RecordClient::new(&backend.base_url, "cma-service").unwrap(),
            app.state.keys.clone(),
            app.state.providers.clone(),
        );
        Self {
            app,
            backend,
            store,
        }
    }

    async fn channel(&self) -> Arc<ResolvedChannel> {
        let id = self.app.fixture.id.as_str();
        self.app.state.resolver.resolve(id).await.unwrap()
    }

    /// Prepare and bind a card for `reference`'s consumer.
    async fn card(&self, reference: &EventRef) -> (PreparedCard, MessageRef) {
        let event = reference.open(&self.app.state.keys, now()).unwrap().payload;
        let message: OutboundMessage = serde_json::from_value(json!({"text": "Approve?",
            "blocks": [{"type": "buttons", "id": "decision",
                "options": [{"id": "yes", "label": "Yes"}, {"id": "no", "label": "No"}]}]}))
        .unwrap();
        let channel = self.channel().await;
        let mut prepared = self
            .store
            .prepare(&channel, &event, &message)
            .await
            .unwrap();
        let destination = Destination {
            outbound_id: uuid::Uuid::new_v4().to_string(),
            platform_id: "42".into(),
        };
        let reference = self.store.bind(&mut prepared, destination).await.unwrap();
        (prepared, reference)
    }

    fn blob(&self, id: &str) -> Vec<u8> {
        self.backend.records()[id].blob.clone()
    }

    /// Click the card's first button with `admit` as the admission gate.
    async fn click(
        &self,
        prepared: &PreparedCard,
        admit: &cmaeg::interactions::Admit,
    ) -> Result<Value, Error> {
        let data = prepared.markup["inline_keyboard"][0][0]["callback_data"]
            .as_str()
            .unwrap();
        let mut click = self.app.activity.clone();
        click.event_id = "click-1".parse().unwrap();
        let channel = self.channel().await;
        let (result, _) = self
            .store
            .consume(&channel, &click, "route-1", "callback-1", data, "42", admit)
            .await?;
        Ok(json!(result.values))
    }
}

#[tokio::test]
async fn a_consumers_card_is_read_through_its_binding_sealed_as_ic_and_referenced_for_it() {
    let cards = Cards::new().await;
    let reference = issue(&cards.app, &reviewer(), &reviewer_key(), None).await;
    let before = cards.app.fake.log().len();
    let (prepared, message_ref) = cards.card(&reference).await;
    let reads: Vec<_> = cards.app.fake.log()[before..]
        .iter()
        .filter(|request| request.method == "GET")
        .cloned()
        .collect();
    assert_eq!(reads.len(), 1);
    assert!(reads[0]
        .path
        .starts_with(&format!("/api/v1/cmaeg/bindings/{REVIEWER_BINDING}/")));
    let credential = format!("Bearer {}", reviewer_key());
    assert_eq!(reads[0].authorization.as_deref(), Some(credential.as_str()));
    assert_eq!(prepared.card.consumer, Some(reviewer()));
    let keys = &cards.app.state.keys;
    let payload = message_ref.open(keys).unwrap().payload;
    assert_eq!(payload.consumer, Some(reviewer()));
    let id = &prepared.card.id;
    let blob = cards.blob(id);
    assert!(blob.starts_with(b"cmaeg1.ic."));
    // An image that predates consumers opens only `in`, and refuses this card.
    assert!(open_interaction(keys, id, &blob).is_err());
    assert!(open_consumer_interaction(keys, id, &blob).is_ok());
}

#[tokio::test]
async fn a_click_the_gate_refuses_leaves_the_card_unconsumed() {
    let cards = Cards::new().await;
    let reference = issue(&cards.app, &reviewer(), &reviewer_key(), None).await;
    let (prepared, _) = cards.card(&reference).await;
    let id = &prepared.card.id;
    let refused = cards
        .click(&prepared, &|consumer: Option<&ConsumerId>| {
            assert_eq!(consumer, Some(&reviewer()));
            false
        })
        .await;
    assert!(matches!(refused, Err(Error::Denied)));
    // A caller may impose an additional admission restriction.
    let production = cards.click(&prepared, &|consumer| consumer.is_none()).await;
    assert!(matches!(production, Err(Error::Denied)));
    let (stored, refused_version) = cards.store.load(id).await.unwrap();
    assert!(
        stored.consumed.is_none(),
        "a refused click consumed the card"
    );
    let admitted = cards
        .click(&prepared, &|consumer| consumer == Some(&reviewer()))
        .await
        .unwrap();
    assert_eq!(admitted["selected"], json!(["yes"]));
    let (stored, version) = cards.store.load(id).await.unwrap();
    assert!(stored.consumed.is_some());
    assert!(version > refused_version);
}

#[tokio::test]
async fn a_replacement_keeps_its_consumer_and_refuses_a_reference_for_another() {
    let cards = Cards::new().await;
    let reference = issue(&cards.app, &reviewer(), &reviewer_key(), None).await;
    let (prepared, message_ref) = cards.card(&reference).await;
    cards
        .click(&prepared, &|consumer| consumer.is_some())
        .await
        .unwrap();
    let keys = &cards.app.state.keys;
    let payload = message_ref.open(keys).unwrap().payload;
    let channel = cards.channel().await;
    let message: OutboundMessage = serde_json::from_value(json!({"text": "Done"})).unwrap();
    let foreign = MessageRefPayload {
        discord_message: None,
        received_reaction: None,
        native_lark_effect: None,
        whatsapp_reaction: None,
        lark_effect: None,
        telegram_effect: None,
        consumer: None,
        ..payload.clone()
    };
    assert!(matches!(
        cards.store.replacement(&channel, &foreign, &message).await,
        Err(Error::Denied)
    ));
    let next = cards
        .store
        .replacement(&channel, &payload, &message)
        .await
        .unwrap();
    assert_eq!(next.card.consumer, Some(reviewer()));
    assert!(cards.blob(&next.card.id).starts_with(b"cmaeg1.ic."));
}

#[tokio::test]
async fn a_default_card_keeps_its_bytes_and_in_kind() {
    let cards = Cards::new().await;
    let reference = cards.app.event_ref.clone();
    let (prepared, message_ref) = cards.card(&reference).await;
    assert_eq!(prepared.card.consumer, None);
    let keys = &cards.app.state.keys;
    assert_eq!(message_ref.open(keys).unwrap().payload.consumer, None);
    let id = &prepared.card.id;
    let blob = cards.blob(id);
    assert!(blob.starts_with(b"cmaeg1.in."));
    let stored: Value =
        serde_json::from_slice(&open_interaction(keys, id, &blob).unwrap().payload).unwrap();
    let fields: BTreeSet<&str> = stored
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let legacy = [
        "actor",
        "channel_id",
        "chat",
        "consumed",
        "destination",
        "event_id",
        "expires_at",
        "id",
        "message",
        "owner",
        "reference",
        "revision",
        "route_id",
        "superseded",
        "thread",
    ];
    assert_eq!(fields, legacy.into_iter().collect());
    assert!(cards
        .click(&prepared, &|consumer| consumer.is_none())
        .await
        .is_ok());
}
