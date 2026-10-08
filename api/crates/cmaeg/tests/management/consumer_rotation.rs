//! Management key rotation revokes already minted references at the next lookup.
use super::{consumer_support::*, *};
use cmaeg::{
    auth::AgentCaller,
    delivery::router::ShardedScheduler,
    tools::{MediaDownloads, ToolsState},
    turns::TurnTracker,
};
use cmaeg_core::{
    ids::{ConsumerId, ConversationId, EventId},
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use sha2::{Digest, Sha256};

fn caller(key: &str) -> AgentCaller {
    AgentCaller {
        key_hash: Sha256::digest(key.as_bytes()).into(),
    }
}

#[tokio::test]
async fn rotation_revokes_the_old_key_on_existing_refs_without_changing_conversation_identity() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let response = added(&gateway, &id, 1, "reviewer").await;
    let consumer = ConsumerId::parse(response["consumer"]["id"].as_str().unwrap()).unwrap();
    let before = gateway.stored(&record).0;
    let conversation = ConversationId::derive_for_consumer(
        &key_ring(1),
        &id,
        before.partition.version,
        &consumer,
        "test",
    );
    let reference = EventRef::seal(
        &EventRefPayload {
            v: 1,
            activity_sha256: None,
            channel_id: id.clone(),
            record_id: record.clone(),
            event_id: EventId::new("event-key-rotation").unwrap(),
            partition_key: "test".into(),
            conversation_id: conversation.clone(),
            expires_at: time::OffsetDateTime::now_utc().unix_timestamp() + 600,
            reply: ReplyAddress::None,
            consumer: Some(consumer.clone()),
            lineage: None,
        },
        &key_ring(1),
    )
    .unwrap();
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let tools = ToolsState {
        resolver: gateway.state.resolver.clone(),
        keys: gateway.state.master_keys.clone(),
        providers: gateway.state.providers.clone(),
        delivery: Arc::new(ShardedScheduler::new(sender)),
        tracker: TurnTracker::new(gateway.state.readiness.clone()),
        public_origin: gateway.state.public_url.clone(),
        media: None,
        downloads: Arc::new(MediaDownloads::default()),
        multi_consumer_enabled: true,
    };
    tools
        .authorize_event(
            caller("fixture-key-reviewer"),
            &reference,
            "verifyEventContext",
        )
        .await
        .unwrap();
    let request = json!({"expected_version":3,"name":"reviewer","credentials":{"agent_key":"rotated-fixture-key"}});
    assert_eq!(
        gateway
            .update(&id, consumer.as_str(), &request)
            .await
            .status(),
        200
    );
    let old = tools
        .authorize_event(
            caller("fixture-key-reviewer"),
            &reference,
            "verifyEventContext",
        )
        .await
        .unwrap_err();
    assert_eq!(old.status, 403);
    assert_eq!(
        old.detail.as_deref(),
        Some("agent key does not authorize this channel")
    );
    let (_, payload, authorized) = tools
        .authorize_event(
            caller("rotated-fixture-key"),
            &reference,
            "verifyEventContext",
        )
        .await
        .unwrap();
    assert_eq!(authorized, consumer);
    assert_eq!(payload.conversation_id, conversation);
    let after = gateway.stored(&record).0;
    assert_eq!(
        before
            .consumer(Some(&consumer))
            .unwrap()
            .provider
            .binding_id,
        after.consumer(Some(&consumer)).unwrap().provider.binding_id
    );
    assert_eq!(
        ConversationId::derive_for_consumer(
            &key_ring(1),
            &id,
            after.partition.version,
            &consumer,
            "test"
        ),
        conversation
    );
    assert_eq!(
        gateway.remove(&id, consumer.as_str(), 4).await.status(),
        200
    );
    assert_eq!(
        tools
            .authorize_event(
                caller("rotated-fixture-key"),
                &reference,
                "verifyEventContext"
            )
            .await
            .unwrap_err()
            .status,
        403
    );
    gateway.close().await;
}

#[derive(Clone, Default)]
struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn trace_logs_cover_bind_rebind_unbind_without_keys_profiles_or_channel_ids() {
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let gateway = Gateway::start(true).await;
    let (id, _, _) = gateway.seed(|_| {});
    let mut request = body(1, "cma_codex", "reviewer");
    request["profile"]["context"]["system_prompt"] =
        "private profile text for log regression".into();
    let response = gateway.add(&id, &request).await;
    assert_eq!(response.status(), 201);
    let response: serde_json::Value = response.json().await.unwrap();
    let consumer = response["consumer"]["id"].as_str().unwrap();
    assert_eq!(gateway.update(&id,consumer,&json!({"expected_version":3,"name":"reviewer","credentials":{"agent_key":"rotated-fixture-key"}})).await.status(),200);
    assert_eq!(gateway.remove(&id, consumer, 4).await.status(), 200);
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    for required in [
        "TRACE",
        "operation=\"bind\"",
        "operation=\"rebind\"",
        "consumer provider unbind completed",
    ] {
        assert!(logs.contains(required), "missing {required}");
    }
    for forbidden in [
        "fixture-key-reviewer",
        "rotated-fixture-key",
        "private profile text for log regression",
        id.as_str(),
    ] {
        assert!(
            !logs.contains(forbidden),
            "sensitive value reached trace log"
        );
    }
    gateway.close().await;
}
