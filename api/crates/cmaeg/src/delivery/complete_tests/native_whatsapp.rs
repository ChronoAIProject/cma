use super::*;
use crate::{
    ingress::routing_store::test_server::Server,
    whatsapp::{self, receipts::Store},
};
use reqwest::Method;

struct Graph(Mutex<Vec<serde_json::Value>>);
impl whatsapp::Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, Method::POST);
        assert_eq!(url.path(), "/v25.0/34567/messages");
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        Box::pin(async {
            Ok(HttpReply::new(StatusCode::OK, serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.confirmed"}]})).unwrap()))
        })
    }
}

#[tokio::test]
async fn native_whatsapp_delivery_requires_current_phone_and_persists_confirmed_receipt() {
    for revoked in [false, true] {
        let (mut sender, relay, mut channel, mut event) = setup();
        let server = Server::start_realtime().await;
        let store = Arc::new(Store::new(server.records.clone(), sender.keys.clone()));
        let graph = Arc::new(Graph(Mutex::default()));
        sender.relay = sender.relay.with_whatsapp(
            whatsapp::Client::production()
                .unwrap()
                .with_transport(graph.clone()),
            store.clone(),
        );
        let definition = &mut Arc::make_mut(&mut channel).definition;
        definition.sources = if revoked {
            vec![]
        } else {
            vec![whatsapp::tests::source()]
        };
        definition.secrets.whatsapp = Some(whatsapp::tests::credentials());
        event.reply = ReplyAddress::Whatsapp {
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
            recipient_id: "447700900123".into(),
            message_id: "wamid.original".into(),
        };
        let gate = RecordingGate::new(true);
        let trace = tracing::Span::none();
        let ctx = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let (receipt, _) = sender
            .reply(&ctx, &event, &message("native reply".into()))
            .await;
        if revoked {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
            assert!(graph.0.lock().unwrap().is_empty());
            assert!(gate.calls().is_empty());
        } else {
            assert!(matches!(
                receipt.delivery,
                DeliveryOutcome::Confirmed { .. }
            ));
            let source = cmaeg_core::activity::SourceKind::Whatsapp {
                app_id: "12345".into(),
                waba_id: "23456".into(),
                phone_number_id: "34567".into(),
            };
            let receipt = store
                .load(
                    &event.channel_id,
                    &source,
                    "447700900123",
                    "wamid.confirmed",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(receipt.original.event_id, event.event_id);
            assert_eq!(receipt.partition_key, event.partition_key);
            assert_eq!(receipt.conversation_id, event.conversation_id);
            assert_eq!(receipt.consumer.as_str(), "default");
            assert_eq!(graph.0.lock().unwrap().len(), 1);
            assert_eq!(
                gate.calls(),
                ["enter_route", "attempt", "commit", "progress"]
            );
        }
        assert!(relay.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_delivery_never_resends_after_receipt_storage_loss() {
    let (mut sender, _, mut channel, mut event) = setup();
    let graph = Arc::new(Graph(Mutex::default()));
    let records = crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap();
    sender.relay = sender.relay.with_whatsapp(
        whatsapp::Client::production()
            .unwrap()
            .with_transport(graph.clone()),
        Arc::new(Store::new(records, sender.keys.clone())),
    );
    let definition = &mut Arc::make_mut(&mut channel).definition;
    definition.sources = vec![whatsapp::tests::source()];
    definition.secrets.whatsapp = Some(whatsapp::tests::credentials());
    event.reply = ReplyAddress::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
        recipient_id: "447700900123".into(),
        message_id: "wamid.original".into(),
    };
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender
        .reply(&ctx, &event, &message("native reply".into()))
        .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
    assert_eq!(graph.0.lock().unwrap().len(), 1);
}
