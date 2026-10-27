use super::native_whatsapp_tests::{media_setup, send_media};
use super::*;
use crate::whatsapp::{
    self,
    outbound::tests::{answer, confirmed},
    templates::tests::rich,
};

#[tokio::test]
async fn native_whatsapp_template_gate_is_independent_and_preflight_never_commits_invalid_messages()
{
    for defect in 0..9 {
        let (mut sender, mut channel, event, graph, _server) = media_setup(vec![]).await;
        sender.relay = sender
            .relay
            .with_whatsapp_templates(true)
            .with_native_whatsapp_templates(defect != 0);
        let mut message = rich();
        let gate = RecordingGate::new(defect != 8);
        match defect {
            0 | 8 => {}
            1 => Arc::make_mut(&mut channel).definition.sources.clear(),
            2 => Arc::make_mut(&mut channel).definition.secrets.whatsapp = None,
            3 => message.text = Some("cannot mix".into()),
            4 => message.thread_reply = true,
            5 => message.whatsapp_template.as_mut().unwrap().name = "Invalid".into(),
            6 => sender.relay.whatsapp_receipts = None,
            7 => message
                .native_attachments
                .push(whatsapp::outbound::tests::attachment("image/png")),
            _ => unreachable!(),
        }
        let receipt = send_media(&sender, &channel, &event, &message, &gate).await;
        assert!(
            matches!(
                receipt.delivery,
                DeliveryOutcome::Unsupported | DeliveryOutcome::Rejected { .. }
            ),
            "{defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty());
        assert!(receipt.message_ref.is_none());
        if defect != 8 {
            assert!(gate.calls().is_empty());
        }
    }
}

#[tokio::test]
async fn native_whatsapp_template_media_uses_existing_public_link_policy_before_send_gate() {
    for link in [
        "https://127.0.0.1/a",
        "https://169.254.169.254/a",
        "https://[::1]/a",
        "https://localhost./a",
        "https://files.internal./a",
        "https://files.local/a",
        "https://files.svc/a",
        "https://files.cluster.local/a",
    ] {
        let (mut sender, channel, event, graph, _server) = media_setup(vec![]).await;
        sender.relay = sender.relay.with_native_whatsapp_templates(true);
        let message =
            whatsapp::templates::tests::message(json!({"type":"image","image":{"link":link}}));
        let gate = RecordingGate::new(true);
        let receipt = send_media(&sender, &channel, &event, &message, &gate).await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }),
            "{link}"
        );
        assert!(gate.calls().is_empty());
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_template_confirmation_requires_exact_recipient_and_single_platform_id() {
    for response in [
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"other"}],"messages":[{"id":"wamid.sent"}]}),
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[]}),
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.a"},{"id":"wamid.b"}]}),
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":""}]}),
        json!({"error":{"code":1}}),
    ] {
        let (mut sender, channel, event, graph, _server) =
            media_setup(vec![answer(response)]).await;
        sender.relay = sender.relay.with_native_whatsapp_templates(true);
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &rich(),
            &RecordingGate::new(true),
        )
        .await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(receipt.message_ref.is_none());
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn native_whatsapp_template_transport_and_receipt_uncertainty_never_resend() {
    for failure in 0..4 {
        let response = match failure {
            0 => Err(TransportError::Unavailable),
            1 => Ok(HttpReply::new(StatusCode::BAD_GATEWAY, b"{}".to_vec())),
            2 => Ok(HttpReply::new(StatusCode::OK, b"not json".to_vec())),
            3 => confirmed("wamid.template"),
            _ => unreachable!(),
        };
        let (mut sender, channel, event, graph, _server) = media_setup(vec![response]).await;
        sender.relay = sender.relay.with_native_whatsapp_templates(true);
        if failure == 3 {
            sender.relay.whatsapp_receipts = Some(Arc::new(whatsapp::receipts::Store::new(
                crate::records::RecordClient::new("http://127.0.0.1:1", "fixture").unwrap(),
                sender.keys.clone(),
            )));
        }
        let receipt = send_media(
            &sender,
            &channel,
            &event,
            &rich(),
            &RecordingGate::new(true),
        )
        .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
            "{failure}"
        );
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
        assert!(receipt.message_ref.is_none());
    }
}
