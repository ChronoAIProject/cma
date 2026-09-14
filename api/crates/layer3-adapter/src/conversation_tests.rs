use super::*;
use crate::AdapterError;

#[test]
fn updates_are_the_published_acp_schema_types() {
    let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello")));
    let json = serde_json::to_value(&update).expect("ACP update serializes");
    assert_eq!(json["sessionUpdate"], "agent_message_chunk");
    let decoded: SessionUpdate = serde_json::from_value(json).expect("ACP update deserializes");
    assert!(matches!(decoded, SessionUpdate::AgentMessageChunk(_)));
}

#[test]
fn stop_reasons_keep_the_acp_wire_vocabulary() {
    for (reason, expected) in [
        (TurnStopReason::EndTurn, "end_turn"),
        (TurnStopReason::MaxTokens, "max_tokens"),
        (TurnStopReason::MaxTurnRequests, "max_turn_requests"),
        (TurnStopReason::Refusal, "refusal"),
        (TurnStopReason::Cancelled, "cancelled"),
    ] {
        assert_eq!(
            serde_json::to_value(reason).expect("stop reason serializes"),
            expected
        );
    }
}

#[tokio::test]
async fn conversation_input_waits_for_a_local_write_receipt() {
    let sender = ConversationInputSender::spawn(1, |_| async { Ok(()) });
    assert!(matches!(
        sender.send_classified(ConversationInput::Cancel).await,
        ConversationSendOutcome::WriteConfirmed
    ));
}

#[tokio::test]
async fn writer_failure_is_ambiguous_and_retires_that_generation() {
    let sender = ConversationInputSender::spawn(1, |_| async {
        Err(AdapterError::Rejected("socket refused the frame".into()))
    });
    assert!(matches!(
        sender.send_classified(ConversationInput::Cancel).await,
        ConversationSendOutcome::Ambiguous(AdapterError::Rejected(detail))
            if detail.contains("socket refused")
    ));
    tokio::task::yield_now().await;
    assert!(matches!(
        sender.send(ConversationInput::Cancel).await,
        Err(AdapterError::Unavailable(_))
    ));
}
