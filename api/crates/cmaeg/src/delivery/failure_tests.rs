use super::*;
use crate::delivery::{
    custom::CustomDelivery, group::MAX_EVENTS, pacing::Pacing, relay_client::RelayClient,
    sync::SyncRegistry,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{channel::ChannelDefinition, ids::ChannelId, seal::ChannelPointer};
use futures_util::StreamExt;
use std::{io::Write, sync::Mutex, time::Duration};

const REVIEWER: &str = "csm_00000000000a";

#[test]
fn only_bounded_provider_codes_are_kept() {
    for code in ["rate_limited", "e5", &"x".repeat(64)] {
        assert_eq!(provider_code(Some(code.into())).as_deref(), Some(code));
    }
    for code in [
        "",
        "Upper",
        "with space",
        "dash-ed",
        "token=secret",
        &"x".repeat(65),
    ] {
        assert_eq!(provider_code(Some(code.into())), None, "{code:?}");
    }
    assert_eq!(provider_code(None), None);
}

fn channel() -> Arc<ResolvedChannel> {
    let definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    Arc::new(ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.failure-fixture").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: std::time::Instant::now(),
        version: 1,
    })
}

fn payload(channel: &ResolvedChannel, event_id: &str, reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: event_id.parse().unwrap(),
        partition_key: event_id.into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply,
        consumer: Some(ConsumerId::parse(REVIEWER).unwrap()),
        lineage: None,
    }
}

fn sender(waiters: Arc<SyncRegistry>) -> Sender {
    Sender {
        custom: CustomDelivery::new("https://gateway.example".into(), waiters),
        keys: Arc::new(
            cmaeg_core::seal::MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32])))
                .unwrap(),
        ),
        relay: RelayClient::new("https://nyx.example", Duration::from_secs(1)).unwrap(),
        pacing: Pacing::default(),
        consumed: Mutex::default(),
    }
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// At the cap, a Sync-origin failure still reaches its waiter, and only
/// then is it dropped: no entry, no notice.
#[tokio::test]
async fn the_failure_frame_is_emitted_before_capacity_is_checked() {
    let logs = Logs::default();
    let writer = logs.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish(),
    );
    let channel = channel();
    let mut groups = Groups::default();
    for index in 0..MAX_EVENTS {
        let live = payload(&channel, &format!("msg-{index}"), ReplyAddress::None);
        let id = key(live.channel_id.as_str(), live.event_id.as_str());
        groups.join(id, &channel, &live).0.terminal_at = None;
    }
    let waiters = Arc::new(SyncRegistry::default());
    let (waiter, frames) = waiters.register().unwrap();
    let sender = sender(waiters);
    let failed = payload(&channel, "msg-new", ReplyAddress::Sync { waiter });
    let failure = ConsumerFailure {
        channel: channel.clone(),
        payload: failed.clone(),
        consumer: ConsumerId::parse(REVIEWER).unwrap(),
        code: "provider_unavailable",
        provider_code: Some("Bearer secret".into()),
    };
    consumer_failed(&mut groups, &sender, &DeliveryConfig::default(), failure).await;
    let frames: Vec<_> = tokio::time::timeout(Duration::from_secs(5), frames.stream().collect())
        .await
        .expect("the terminal frame ended the stream");
    assert_eq!(frames.len(), 1, "exactly the failure frame");
    assert_eq!(groups.entries(), MAX_EVENTS, "the failure took no entry");
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("delivery event capacity reached"), "{logs}");
    assert!(logs.contains("fan-out consumer failed"), "{logs}");
    assert!(
        !logs.contains("secret"),
        "an invalid provider code is dropped: {logs}"
    );
}

#[tokio::test]
async fn a_failure_whose_consumer_disagrees_with_its_reference_is_dropped() {
    let channel = channel();
    let waiters = Arc::new(SyncRegistry::default());
    let (waiter, frames) = waiters.register().unwrap();
    let sender = sender(waiters.clone());
    let mut groups = Groups::default();
    let failure = ConsumerFailure {
        channel: channel.clone(),
        payload: payload(&channel, "msg-1", ReplyAddress::Sync { waiter }),
        consumer: ConsumerId::default_consumer(),
        code: "provider_unavailable",
        provider_code: None,
    };
    consumer_failed(&mut groups, &sender, &DeliveryConfig::default(), failure).await;
    assert_eq!(groups.entries(), 0);
    assert_eq!(waiters.len(), 1, "no frame was emitted");
    drop(frames);
}
