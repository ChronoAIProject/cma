//! Source-derived native NyxBot contract, without native platform traffic.
//! NyxID c2a5c8fa9a32f461ffeff55fe0d007ec76de7304:
//! handlers/nyxbot_native_responses.rs and services/nyxbot_response_recovery.rs.
use super::support::{frame, key, Config, Server};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::providers::{
    ProviderClient, ProviderError, ProviderRegistry, RecoveryIdentity, ResponsesEvent,
};
use cmaeg_core::{
    ids::{ConversationId, EventId, ProviderSlug, RecordId},
    seal::{ChannelPointer, EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

const RESPONSE: &str = "resp_accepted";
const NATIVE: &str = "nyxbot_0123456789abcdef0123456789abcdef";

fn provider() -> ProviderSlug {
    ProviderSlug::new("nyxbot").unwrap()
}
fn identity() -> RecoveryIdentity {
    RecoveryIdentity {
        response_id: RESPONSE.into(),
        native_agent_id: None,
        metadata: BTreeMap::from([
            ("cmaeg.channel".into(), "0123456789abcdef".into()),
            ("cmaeg.event".into(), "discord:100:500".into()),
            ("cmaeg.partition".into(), "abcdef0123456789".into()),
            ("cmaeg.actor".into(), "7".into()),
        ]),
    }
}
fn item() -> Value {
    json!({"id":"msg_resp_accepted","type":"message","role":"assistant",
        "status":"completed","content":[{"type":"output_text","text":"Native answer"}]})
}
fn snapshot(status: &str) -> Value {
    let output = if status == "completed" {
        vec![item()]
    } else {
        vec![]
    };
    let mut value = json!({"id":RESPONSE,"object":"response","status":status,
        "conversation":{"id":NATIVE},"metadata":identity().metadata,"output":output});
    if status == "failed" {
        value["error"] = json!({"code":"provider_turn_failed",
            "message":"NyxBot could not finish this turn."});
    }
    value
}
fn lifecycle(sequence: u64, kind: &str, status: &str) -> String {
    frame(
        Some(&sequence.to_string()),
        json!({"type":kind,"sequence_number":sequence,"response":snapshot(status)}),
    )
}
fn completion() -> String {
    frame(
        Some("2"),
        json!({"type":"response.output_item.done","sequence_number":2,
            "output_index":0,"item":item()}),
    ) + &lifecycle(3, "response.completed", "completed")
}
fn client(server: &Server) -> ProviderClient {
    let registry = json!([{"slug":"nyxbot","base_url":server.base,"kind":"responses_http",
        "max_inflight":1,"paths":{"agent_card":"/api/v1/nyxbot/agent-card",
        "binding":"/api/v1/nyxbot/bindings/{binding_id}",
        "conversation":"/api/v1/nyxbot/bindings/{binding_id}/conversations/{conversation_id}",
        "responses":"/api/v1/nyxbot/responses","recovery":{
            "response":"/api/v1/nyxbot/responses/{response_id}",
            "events":"/api/v1/nyxbot/responses/{response_id}/events"}}}]);
    ProviderClient::new(
        ProviderRegistry::parse(&registry.to_string()).unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(1),
        1,
    )
    .unwrap()
}

#[tokio::test]
async fn native_snapshot_and_replay_use_exact_identity_output_and_exclusive_cursor() {
    let server = Server::new(Config {
        snapshot: snapshot("completed"),
        frames: lifecycle(0, "response.created", "queued")
            + &lifecycle(1, "response.in_progress", "in_progress")
            + &completion(),
        ..Default::default()
    })
    .await;
    let client = client(&server);
    let accepted = client
        .response_snapshot(&provider(), &identity(), &key())
        .await
        .unwrap();
    assert_eq!(accepted.native_agent_id, NATIVE);
    let mut pinned = identity();
    pinned.native_agent_id = Some(accepted.native_agent_id);
    let mut stream = client
        .replay_response(&provider(), &pinned, None, &key())
        .await
        .unwrap();
    for sequence in 0..4 {
        let event = stream.events.recv().await.unwrap().unwrap();
        assert_eq!(event.sequence_number, sequence);
        assert_eq!(event.response_id, RESPONSE);
        if sequence == 3 {
            assert!(matches!(event.event, ResponsesEvent::Completed { .. }));
            assert_eq!(event.raw["response"]["output"][0], item());
        }
    }
    assert!(stream.events.recv().await.is_none());
    server.config.lock().unwrap().frames = completion();
    let mut resumed = client
        .replay_response(&provider(), &pinned, Some(1), &key())
        .await
        .unwrap();
    for sequence in [2, 3] {
        assert_eq!(
            resumed
                .events
                .recv()
                .await
                .unwrap()
                .unwrap()
                .sequence_number,
            sequence
        );
    }
    assert!(resumed.events.recv().await.is_none());
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].uri, "/api/v1/nyxbot/responses/resp_accepted");
    assert_eq!(
        seen[2].uri,
        "/api/v1/nyxbot/responses/resp_accepted/events?starting_after=1"
    );
    assert!(seen
        .iter()
        .all(|r| r.method == "GET" && r.authorization == "Bearer fixture-current-agent-key"));
}

#[tokio::test]
async fn native_observation_failure_does_not_complete_or_resubmit_the_turn() {
    let server = Server::new(Config {
        frames: lifecycle(0, "response.created", "queued")
            + &frame(None, json!({"type":"error","code":"provider_unavailable"})),
        ..Default::default()
    })
    .await;
    let client = client(&server);
    let mut first = client
        .replay_response(&provider(), &identity(), None, &key())
        .await
        .unwrap();
    assert_eq!(
        first.events.recv().await.unwrap().unwrap().sequence_number,
        0
    );
    assert!(matches!(
        first.events.recv().await.unwrap(),
        Err(ProviderError::Unavailable(_))
    ));
    assert!(first.events.recv().await.is_none());
    server.config.lock().unwrap().frames = lifecycle(1, "response.in_progress", "in_progress")
        + &lifecycle(2, "response.failed", "failed");
    let mut resumed = client
        .replay_response(&provider(), &identity(), Some(0), &key())
        .await
        .unwrap();
    assert_eq!(
        resumed
            .events
            .recv()
            .await
            .unwrap()
            .unwrap()
            .sequence_number,
        1
    );
    let failure = resumed.events.recv().await.unwrap().unwrap();
    assert_eq!(failure.sequence_number, 2);
    assert!(matches!(failure.event, ResponsesEvent::Failed { .. }));
    assert!(resumed.events.recv().await.is_none());
    assert!(server
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.method == "GET"));
}

#[test]
fn native_discord_reference_cannot_enter_short_command_metadata() {
    let keys = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap();
    // Even the smallest admissible pointer and shorter-than-real native IDs
    // exceed the 512-byte metadata cutoff in turns/metadata.rs. The capability
    // stays in encrypted event_context; the four command fields remain exact.
    let pointer = ChannelPointer {
        v: 1,
        record_url: "https://a".into(),
        record_id: RecordId::new("x").unwrap(),
    };
    pointer.validate().unwrap();
    let channel_id = pointer.seal_id(&keys).unwrap();
    let payload = EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: Some("0".repeat(64)),
        channel_id,
        record_id: pointer.record_id,
        event_id: EventId::new("x").unwrap(),
        partition_key: "x".into(),
        conversation_id: ConversationId::new("x").unwrap(),
        expires_at: 0,
        reply: ReplyAddress::Discord {
            bot_id: "1".into(),
            channel_id: "1".into(),
            message_id: "1".into(),
            thread_id: None,
        },
        consumer: None,
        lineage: None,
    };
    assert!(EventRef::seal(&payload, &keys).unwrap().as_str().len() > 512);
}
