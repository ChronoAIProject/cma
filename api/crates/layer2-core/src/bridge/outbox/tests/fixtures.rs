use super::*;

pub(super) fn bridge() -> BridgeRecord {
    serde_json::from_value(json!({
        "id":"brg_test","owner":{"kind":"person","subject":"owner"},"revision":1,"config_revision":1,
        "state":{"kind":"enabled"},"emergency":{"ingress_stopped":false,"execution_stopped":false,"outbound_stopped":false},
        "created_at":"2026-09-15T06:00:00Z","updated_at":"2026-09-15T06:00:00Z","created_by":{"kind":"human","subject":"owner"},
        "generations":[{"config_revision":1,"created_at":"2026-09-15T06:00:00Z","actor":{"kind":"human","subject":"owner"},"previous_work":null,
            "config":{"display_name":"Outbox fixture","connector":{"issuer":"https://nyx.example","base_url":"https://nyx.example","bot_id":"bot","platform":"telegram","route_ids":["route"],"key_id":"key","custody":"brsec_00000000000000000000000000000001","callback_url":"https://cma.example/bridge"},
            "partition":"fixed_shared_agent","placement":{"kind":"existing_agent","target":{"sandbox_id":"sandbox","workspace_id":"workspace","worktree_id":"worktree","agent_id":"agent"}},
            "admission":{"kind":"all"},"reply":"final_response","limits":{"max_partitions":2,"max_sandboxes":2,"max_queued_messages":5,"max_messages_per_minute":30,"max_payload_bytes":4096,"inbox_retention_seconds":3600,"reply_secret_retention_seconds":600}}
        }]
    })).unwrap()
}
pub(super) fn source() -> OutboxSource {
    let bridge = bridge();
    let config = bridge.current().unwrap().config.clone();
    let BridgePlacementPolicy::ExistingAgent { target } = &config.placement else {
        unreachable!()
    };
    OutboxSource {
        inbox_id: "1".repeat(64),
        inbox_sha256: "2".repeat(64),
        allocation_id: "allocation".into(),
        command_id: crate::ids::AgentCommandId("command".into()),
        response_id: crate::ids::AgentResponseId("response".into()),
        admission_sha256: "3".repeat(64),
        owner: bridge.owner.clone(),
        pin: bridge.admission_pin().unwrap(),
        target: target.clone(),
        config,
        message_id: "original-message".into(),
        chat_id: "original-chat".into(),
        source_sha256: "4".repeat(64),
        reply_expires_at_ms: Some(10_000),
        route_expires_at_ms: None,
    }
}
pub(super) fn key() -> DeliveryCredentialRef {
    DeliveryCredentialRef::RetainedKey {
        reference: source().config.connector.custody,
        revision: 1,
        key_state_version: 1,
    }
}
pub(super) fn token() -> DeliveryCredentialRef {
    DeliveryCredentialRef::ReplyToken {
        inbox_id: "1".repeat(64),
        expires_at_ms: 10_000,
    }
}
pub(super) fn scope() -> ChannelReplyTurnAuthorization {
    ChannelReplyTurnAuthorization {
        source: source(),
        namespace: "5".repeat(64),
        capability_incarnation: "session-capability".into(),
        context_reference: "context".into(),
        turn_sequence: 1,
        runtime_generation: 1,
        operation_sequence: 1,
        agent_created_at: "2026-09-15T06:00:00Z".into(),
        creation_fingerprint: "6".repeat(64),
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(10),
    }
}
pub(super) fn intent(text: &str, request_key: &str) -> ChannelReplyToolIntent {
    ChannelReplyToolIntent::prepare(
        scope(),
        ChannelReplyToolRequest {
            request_key: request_key.into(),
            text: text.into(),
        },
    )
    .unwrap()
}
pub(super) fn items(text: &str, single_use: bool) -> Vec<OutboxItem> {
    OutboxItem::from_tool_intent(
        &intent(text, "request"),
        if single_use { token() } else { key() },
        1,
        None,
        100,
    )
    .unwrap()
}
pub(super) fn receipt() -> DeliveryReceipt {
    DeliveryReceipt {
        nyxid_message_id: "nyxid-reply".into(),
        platform_message_id: Some("platform-reply".into()),
    }
}
pub(super) fn send(item: &mut OutboxItem, outcome: DeliveryOutcome) {
    let attempt = item.claim("worker".into(), 101, 1000, None).unwrap();
    item.begin_send("worker", &attempt, 102).unwrap();
    item.complete(&attempt, outcome, 103).unwrap();
}
