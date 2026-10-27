use super::{
    native_whatsapp_tests::{media_setup, send_media},
    *,
};
use crate::whatsapp::{
    self,
    groups::tests::{self as group, answer, confirmed, proofs, Graph, GROUP},
    outbound::tests::attachment,
};
use serde_json::Value;

async fn group_setup(
    replies: Vec<Result<HttpReply, TransportError>>,
) -> (
    Sender,
    Arc<ResolvedChannel>,
    EventRefPayload,
    Arc<Graph>,
    crate::ingress::routing_store::test_server::Server,
) {
    let (mut sender, mut channel, mut event, _, server) = media_setup(vec![]).await;
    let graph = Graph::new(replies);
    sender.relay = sender
        .relay
        .with_native_whatsapp_groups(true)
        .with_native_whatsapp_templates(true)
        .with_whatsapp(
            whatsapp::Client::production()
                .unwrap()
                .with_transport(graph.clone()),
            Arc::new(whatsapp::receipts::Store::new(
                server.records.clone(),
                sender.keys.clone(),
            )),
        );
    Arc::make_mut(&mut channel).definition.sources = vec![group::source_group()];
    event.reply = ReplyAddress::WhatsappGroup(group::reply());
    (sender, channel, event, graph, server)
}
#[tokio::test]
async fn native_whatsapp_group_text_media_and_templates_preserve_exact_destination() {
    for kind in [
        "text",
        "image/png",
        "application/pdf",
        "audio/mpeg",
        "video/mp4",
        "template",
    ] {
        let mut replies = proofs();
        let mut message = message("Hello group".into());
        if kind == "template" {
            message = group::template();
        } else if kind != "text" {
            message.text = None;
            message.native_attachments = vec![attachment(kind)];
            replies.push(answer(json!({"id":"45678"})));
            replies.extend(proofs());
        }
        replies.push(confirmed("wamid.group-sent"));
        let (sender, channel, event, graph, _server) = group_setup(replies).await;
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{kind}: {:?}",
            receipt.delivery
        );
        assert!(receipt.message_ref.is_none());
        let calls = graph.calls.lock().unwrap();
        let (_, url, body) = calls.last().unwrap();
        assert_eq!(url.path(), "/v25.0/34567/messages");
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["recipient_type"], "group");
        assert_eq!(body["to"], GROUP);
        assert_eq!(body["context"]["message_id"], "wamid.original");
        assert_eq!(
            calls
                .iter()
                .filter(|(method, _, _)| *method == reqwest::Method::GET)
                .count(),
            if matches!(kind, "text" | "template") {
                3
            } else {
                6
            }
        );
    }
}
#[tokio::test]
async fn native_whatsapp_group_effects_refuse_disabled_revoked_and_unsupported_paths_before_graph()
{
    for defect in 0..7 {
        let (mut sender, mut channel, event, graph, _server) = group_setup(vec![]).await;
        let mut message = message("Hello".into());
        match defect {
            0 => sender.relay = sender.relay.with_native_whatsapp_groups(false),
            1 => Arc::make_mut(&mut channel).definition.sources = vec![whatsapp::tests::source()],
            2 => Arc::make_mut(&mut channel).definition.secrets.whatsapp = None,
            3 => {
                message.text = None;
                message.native_attachments = vec![attachment("image/webp")];
            }
            4 => message = whatsapp::templates::tests::rich(),
            5 => {
                message = serde_json::from_value(
                    json!({"text":"Choose","blocks":[{"type":"buttons","id":"choose","options":[{"id":"one","label":"One"}]}]}),
                )
                .unwrap()
            }
            6 => {
                sender.relay = sender.relay.with_native_whatsapp_templates(false);
                message = group::template();
            }
            _ => unreachable!(),
        }
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &message,
            &RecordingGate::new(true),
        )
        .await;
        assert!(
            matches!(
                receipt.delivery,
                DeliveryOutcome::Unsupported | DeliveryOutcome::Rejected { .. }
            ),
            "{defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty(), "{defect}");
    }
}
#[tokio::test]
async fn native_whatsapp_group_membership_loss_after_upload_prevents_send() {
    let mut replies = proofs();
    replies.push(answer(json!({"id":"45678"})));
    let mut revoked = group::proof_values();
    revoked[2]["suspended"] = true.into();
    replies.extend(revoked.into_iter().map(answer));
    let (sender, channel, event, graph, _server) = group_setup(replies).await;
    let mut message = message(String::new());
    message.text = None;
    message.native_attachments = vec![attachment("application/pdf")];
    let receipt = send_media(
        &sender,
        &channel,
        &event,
        &message,
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }));
    let calls = graph.calls.lock().unwrap();
    assert_eq!(calls.len(), 7);
    assert_eq!(
        calls
            .iter()
            .filter(|(method, _, _)| *method == reqwest::Method::POST)
            .count(),
        1
    );
    assert!(calls
        .iter()
        .all(|(_, url, _)| !url.path().ends_with("/messages")));
}
#[tokio::test]
async fn native_whatsapp_group_receipt_failure_is_unknown_and_never_resends() {
    let mut replies = proofs();
    replies.push(confirmed("wamid.group-sent"));
    let (mut sender, channel, event, graph, _server) = group_setup(replies).await;
    sender.relay.whatsapp_receipts = Some(Arc::new(whatsapp::receipts::Store::new(
        crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap(),
        sender.keys.clone(),
    )));
    let receipt = send_media(
        &sender,
        &channel,
        &event,
        &message("hello".into()),
        &RecordingGate::new(true),
    )
    .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
    assert_eq!(graph.calls.lock().unwrap().len(), 4);
}
