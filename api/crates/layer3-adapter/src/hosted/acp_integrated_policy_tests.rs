use super::*;
use crate::ConversationSettingsSupport;

#[tokio::test]
async fn combined_hosted_policies_keep_shared_receipts_and_refuse_without_ack_or_terminal() {
    for malformed_json in [false, true] {
        let (mut opened, mut server, _, mut received) = open_policies(true, true).await;
        let (capture, mut prompt_effects) = UpdateRelay::capture();
        opened
            .bridge
            .drive(ConversationInput::Prompt(vec![]), &capture)
            .await
            .unwrap();
        assert_eq!(receive_json(&mut server).await["method"], "session/prompt");

        let mut frame = vec![LANE_RETAINED_OUTPUT];
        frame.extend(retained_bytes(3, &permission(1, "allow")));
        server.send(Message::Binary(frame)).await.unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        let ConversationUpdate::RuntimeOutput { receipt, update } = accepted else {
            panic!("valid input retains its source receipt")
        };
        assert_eq!(receipt.source.cursor, 3);
        let effect = update.unwrap();
        assert!(effect.is_shared());
        assert!(std::ptr::eq(&*effect, &*effect.clone()));
        assert_eq!(request(&effect).request.options.len(), 1);
        receipt.acknowledge().await.unwrap();
        let ack = tokio::time::timeout(Duration::from_secs(2), server.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Binary(ack) = ack else {
            panic!("durable source ACK")
        };
        assert_eq!(ack[0], LANE_OUTPUT_ACK);
        assert_eq!(&ack[1..17], stream_id().as_bytes());
        assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), 3);

        let mut frame = vec![LANE_RETAINED_OUTPUT];
        if malformed_json {
            frame.extend_from_slice(stream_id().as_bytes());
            frame.extend_from_slice(&4_u64.to_be_bytes());
            frame.push(b'{');
        } else {
            frame.extend(retained_bytes(
                4,
                &permission(2, &"x".repeat(MAX_OPTION_BYTES + 1)),
            ));
        }
        server.send(Message::Binary(frame)).await.unwrap();
        if !malformed_json {
            assert_eq!(receive_json(&mut server).await["error"]["code"], -32002);
        }
        tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
            .await
            .unwrap()
            .unwrap();
        assert!(received.try_recv().is_err());
        assert_eq!(
            *opened
                .bridge
                .managed_context_progress()
                .await
                .unwrap()
                .borrow(),
            3,
            "the refused source did not advance durable progress"
        );
        drop(capture);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), prompt_effects.recv())
                .await
                .unwrap()
                .is_none()
        );
        opened.bridge.retire().await;
        opened.relay.abort();
        let _ = opened.relay.await;
    }
}
