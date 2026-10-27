use super::*;
use crate::{
    interactions::{
        whatsapp_native_fixture::{message as controls, Fixture},
        InteractionStore,
    },
    whatsapp,
};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Graph {
    cards: Arc<InteractionStore>,
    records: Arc<crate::ingress::routing_store::test_server::Memory>,
    seen: Mutex<Vec<serde_json::Value>>,
    mode: usize,
    calls: AtomicUsize,
}
impl whatsapp::Transport for Graph {
    fn exchange(
        &self,
        method: reqwest::Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, reqwest::Method::POST);
        assert_eq!(url.path(), "/v25.0/34567/messages");
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(body.clone());
        Box::pin(async move {
            if body["type"] == "interactive" {
                let markup = &body["interactive"];
                let loc = if markup["type"] == "button" {
                    markup["action"]["buttons"][0]["reply"]["id"]
                        .as_str()
                        .unwrap()
                } else {
                    markup["action"]["sections"][0]["rows"][0]["id"]
                        .as_str()
                        .unwrap()
                };
                let id = uuid::Uuid::parse_str(loc.split(':').nth(1).unwrap())
                    .unwrap()
                    .to_string();
                let card = self.cards.load(&id).await.unwrap().0;
                assert!(card.native_whatsapp.is_some());
                assert!(card.destination.is_none());
                if self.mode >= 3 {
                    *self.records.fault.lock().unwrap() =
                        Some(crate::ingress::routing_store::test_server::Fault {
                            id,
                            store: self.mode != 3,
                            lose_read: self.mode == 5,
                            skip: 0,
                        });
                }
            }
            if self.mode == 1 {
                return Err(TransportError::Unavailable);
            }
            let recipient = if self.mode == 2 {
                "447700900999"
            } else {
                "447700900123"
            };
            Ok(HttpReply::new(StatusCode::OK,serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"wa_id":recipient}],"messages":[{"id":"wamid.confirmed"}]})).unwrap()))
        })
    }
}
fn sender(f: &Fixture, mode: usize, enabled: bool) -> (Sender, Arc<Graph>) {
    let (mut sender, _, _, _) = setup();
    let graph = Arc::new(Graph {
        cards: f.store.clone(),
        records: f.server.memory.clone(),
        seen: Mutex::default(),
        mode,
        calls: AtomicUsize::new(0),
    });
    sender.relay = sender
        .relay
        .with_interactions(f.store.clone())
        .with_consumer_interactions(true)
        .with_native_whatsapp_interactions(enabled)
        .with_whatsapp(
            whatsapp::Client::production()
                .unwrap()
                .with_transport(graph.clone()),
            Arc::new(whatsapp::receipts::Store::new(
                f.server.records.clone(),
                sender.keys.clone(),
            )),
        );
    (sender, graph)
}
#[tokio::test]
async fn native_whatsapp_buttons_and_lists_persist_before_send_then_bind_exact_receipt() {
    for named in [false, true] {
        for list in [false, true] {
            let f = Fixture::new(named).await;
            let (sender, graph) = sender(&f, 0, true);
            let gate = RecordingGate::new(true);
            let trace = tracing::Span::none();
            let ctx = SendContext {
                trace: &trace,
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            };
            let (receipt, _) = sender.reply(&ctx, &f.event, &controls(list)).await;
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            let reference = receipt
                .message_ref
                .unwrap()
                .open(&sender.keys)
                .unwrap()
                .payload;
            assert_eq!(reference.consumer, f.event.consumer);
            assert_eq!(reference.platform_message_id, "wamid.confirmed");
            let card = f
                .store
                .load(reference.interaction_record_id.as_ref().unwrap())
                .await
                .unwrap()
                .0;
            assert_eq!(card.destination.unwrap().platform_id, "wamid.confirmed");
            assert_eq!(graph.calls.load(Ordering::SeqCst), 1);
            assert_eq!(f.provider.seen().len(), 1);
            let sent = graph.seen.lock().unwrap();
            assert_eq!(sent[0]["to"], "447700900123");
            assert_eq!(sent[0]["context"]["message_id"], "wamid.original");
            assert_eq!(
                sent[0]["interactive"]["type"],
                if list { "list" } else { "button" }
            );
            assert!(!sent[0].to_string().contains("private-"));
            assert_eq!(
                gate.calls(),
                ["enter_route", "commit", "attempt", "progress"]
            );
        }
    }
}
#[tokio::test]
async fn native_whatsapp_unknown_acceptance_and_binding_failure_never_consume_or_resend() {
    for mode in [1, 2, 3] {
        let f = Fixture::new(false).await;
        let (sender, graph) = sender(&f, mode, true);
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.event, &controls(false)).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            "mode {mode}"
        );
        assert!(receipt.message_ref.is_none());
        assert_eq!(graph.calls.load(Ordering::SeqCst), 1);
        let locator = graph.seen.lock().unwrap()[0]["interactive"]["action"]["buttons"][0]["reply"]
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let activity = f.activity(&locator, "wamid.click");
        let route =
            crate::interactions::whatsapp_authority::route(&activity.source, &activity.actor.id)
                .unwrap();
        assert!(matches!(
            f.store
                .consume_for_dispatch(
                    &f.channel,
                    &activity,
                    &route,
                    "wamid.click",
                    &locator,
                    "wamid.confirmed",
                    None
                )
                .await,
            Err(crate::interactions::Error::Unavailable)
        ));
    }
}
#[tokio::test]
async fn native_whatsapp_disabled_issuance_keeps_text_fallback_and_issued_callbacks() {
    let f = Fixture::new(false).await;
    let card = f.card(false, true).await;
    let (sender, graph) = sender(&f, 0, false);
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender.reply(&ctx, &f.event, &controls(false)).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(receipt.message_ref.is_none());
    assert!(graph
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|body| body["type"] == "text"));
    let locator = Fixture::locator(&card, false);
    let activity = f.activity(&locator, "wamid.click");
    let route =
        crate::interactions::whatsapp_authority::route(&activity.source, &activity.actor.id)
            .unwrap();
    assert!(f
        .store
        .consume_for_dispatch(
            &f.channel,
            &activity,
            &route,
            "wamid.click",
            &locator,
            "wamid.confirmed",
            None
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn native_whatsapp_lost_binding_ack_recovers_exact_authority_without_resend() {
    for mode in [4, 5] {
        let f = Fixture::new(false).await;
        let (sender, graph) = sender(&f, mode, true);
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &f.channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender.reply(&ctx, &f.event, &controls(false)).await;
        if mode == 4 {
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            assert!(receipt.message_ref.is_some());
        } else {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
            assert!(receipt.message_ref.is_none());
        }
        assert_eq!(graph.calls.load(Ordering::SeqCst), 1);
        let locator = graph.seen.lock().unwrap()[0]["interactive"]["action"]["buttons"][0]["reply"]
            ["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let activity = f.activity(&locator, "wamid.click");
        let route =
            crate::interactions::whatsapp_authority::route(&activity.source, &activity.actor.id)
                .unwrap();
        let consume = || {
            f.store.consume_for_dispatch(
                &f.channel,
                &activity,
                &route,
                "wamid.click",
                &locator,
                "wamid.confirmed",
                None,
            )
        };
        let first = consume().await.unwrap();
        let writes = f.server.memory.writes.load(Ordering::SeqCst);
        let replay = consume().await.unwrap();
        assert_eq!(
            serde_json::to_value(first.0).unwrap(),
            serde_json::to_value(replay.0).unwrap()
        );
        assert_eq!(first.1, replay.1);
        assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
        assert_eq!(graph.calls.load(Ordering::SeqCst), 1);
    }
}
