use super::*;
use cmaeg::{
    auth::Creator,
    channels::management,
    delivery::relay_client::{RelayClient, RelayTransport},
    net::exchange::{HttpReply, TransportError},
    outbound::{self, OutboundStatus, PrepareOutboundMessage},
};
use cmaeg_core::{
    channel::{Platform, SourceId},
    message::OutboundMessage,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};
const ROUTE: &str = "91000000-0000-4000-8000-000000000001";

#[derive(Default)]
struct Probe {
    calls: AtomicUsize,
    lost: AtomicBool,
    omit_platform_receipt: AtomicBool,
    blocked: AtomicBool,
    entered: tokio::sync::Notify,
    bodies: Mutex<Vec<serde_json::Value>>,
    timeouts: Mutex<Vec<Duration>>,
}
impl RelayTransport for Probe {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            assert_eq!(url.path(), "/api/v1/channel-relay/send");
            assert_eq!(headers["authorization"], "Bearer nyx_route_secret");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.timeouts.lock().unwrap().push(timeout);
            self.bodies
                .lock()
                .unwrap()
                .push(serde_json::from_slice(&body).unwrap());
            self.entered.notify_one();
            if self.blocked.load(Ordering::SeqCst) {
                futures_util::future::pending::<()>().await;
            }
            if self.lost.load(Ordering::SeqCst) {
                return Err(TransportError::Unavailable);
            }
            if self.omit_platform_receipt.load(Ordering::SeqCst) {
                return Ok(HttpReply::new(
                    StatusCode::OK,
                    br#"{"message_id":"receipt"}"#.to_vec(),
                ));
            }
            Ok(HttpReply::new(
                StatusCode::OK,
                br#"{"message_id":"receipt","platform_message_id":"native-id"}"#.to_vec(),
            ))
        })
    }
}
struct Fixture {
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    state: ManagementState,
    probe: Arc<Probe>,
    channel: String,
    record: String,
}
impl Fixture {
    async fn new(platform: Platform) -> Self {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let probe = Arc::new(Probe::default());
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        state.outbound_messages_writes_enabled = true;
        state.outbound_relay = Some(
            RelayClient::new(&nyxid.base_url, Duration::from_secs(1))
                .unwrap()
                .with_transport(probe.clone())
                .with_native_media(true)
                .with_whatsapp_templates(true),
        );
        let mut body = create_body();
        body["sources"][0]["route_ids"] = json!([ROUTE]);
        body["sources"][0]["platform"] = json!(platform);
        if matches!(platform, Platform::Whatsapp | Platform::Discord) {
            body["sources"][0]["trust_normalized"] = true.into();
        }
        let created = management::create(&state, &owner(), serde_json::from_value(body).unwrap())
            .await
            .unwrap();
        Self {
            provider,
            cma,
            nyxid,
            state,
            probe,
            channel: created.channel_id.as_str().into(),
            record: created.record_id.as_str().into(),
        }
    }
    fn restart(&self) -> ManagementState {
        let mut state = management_state(&self.provider, &self.cma, &self.nyxid, key_ring(1));
        state.outbound_messages_writes_enabled = true;
        state.outbound_relay = self.state.outbound_relay.clone();
        state
    }
    async fn prepare(&self, key: &str) -> outbound::OutboundReceipt {
        outbound::prepare(&self.state, &owner(), &self.channel, key, request())
            .await
            .unwrap()
    }
    fn change_record(&self, id: &str, change: impl FnOnce(&mut serde_json::Value)) {
        let row = self.cma.records().remove(id).unwrap();
        let bytes = if id == self.record {
            cmaeg_core::seal::open_blob(&key_ring(1), &row.blob)
                .unwrap()
                .payload
        } else {
            cmaeg_core::seal::open_interaction(&key_ring(1), id, &row.blob)
                .unwrap()
                .payload
        };
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        change(&mut value);
        let bytes = serde_json::to_vec(&value).unwrap();
        let sealed = if id == self.record {
            cmaeg_core::seal::seal_blob(&key_ring(1), &bytes).unwrap()
        } else {
            cmaeg_core::seal::seal_interaction(&key_ring(1), id, &bytes).unwrap()
        };
        self.cma.seed(id, &row.owner, sealed);
    }
    async fn close(self) {
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}
fn owner() -> Creator {
    Creator {
        subject: "alice".into(),
        token_hash: "hash".into(),
        organization: false,
    }
}
fn message() -> OutboundMessage {
    serde_json::from_value(json!({"text":"hello persisted destination"})).unwrap()
}
fn request() -> PrepareOutboundMessage {
    PrepareOutboundMessage {
        source_id: SourceId::parse("relay_telegram").unwrap(),
        route_id: ROUTE.into(),
        message: message(),
    }
}

#[tokio::test]
async fn outbound_prepare_restart_commit_is_exactly_once_and_content_is_not_stored() {
    let f = Fixture::new(Platform::Telegram).await;
    let first = f.prepare("one").await;
    let row = f.cma.records().remove(&first.delivery_id).unwrap();
    let opened =
        cmaeg_core::seal::open_interaction(&key_ring(1), &first.delivery_id, &row.blob).unwrap();
    assert!(!String::from_utf8_lossy(&opened.payload).contains("hello persisted destination"));
    assert_eq!(first.status, OutboundStatus::Prepared);
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    let restarted = f.restart();
    let replay = outbound::prepare(&restarted, &owner(), &f.channel, "one", request())
        .await
        .unwrap();
    assert_eq!(replay.delivery_id, first.delivery_id);
    let caller = owner();
    let (one, two) = tokio::join!(
        outbound::commit(
            &restarted,
            &caller,
            &f.channel,
            &first.delivery_id,
            message()
        ),
        outbound::commit(&f.state, &caller, &f.channel, &first.delivery_id, message())
    );
    assert_eq!(one.unwrap().status, OutboundStatus::Committed);
    assert_eq!(two.unwrap().status, OutboundStatus::Committed);
    let result = outbound::read(&restarted, &owner(), &f.channel, &first.delivery_id)
        .await
        .unwrap();
    assert_eq!(result.delivery.unwrap().kind(), "confirmed");
    outbound::commit(
        &restarted,
        &owner(),
        &f.channel,
        &first.delivery_id,
        message(),
    )
    .await
    .unwrap();
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.probe.bodies.lock().unwrap()[0]["conversation_id"], ROUTE);
    f.close().await;
}
#[tokio::test]
async fn outbound_owner_destination_payload_and_record_domains_fail_closed() {
    let f = Fixture::new(Platform::Telegram).await;
    let prepared = f.prepare("fenced").await;
    let mut other = owner();
    other.subject = "bob".into();
    assert_eq!(
        outbound::read(&f.state, &other, &f.channel, &prepared.delivery_id)
            .await
            .err()
            .unwrap()
            .status,
        404
    );
    assert!(outbound::commit(
        &f.state,
        &other,
        &f.channel,
        &prepared.delivery_id,
        message()
    )
    .await
    .is_err());
    let mut changed = request();
    changed.route_id = uuid::Uuid::new_v4().to_string();
    assert!(
        outbound::prepare(&f.state, &owner(), &f.channel, "wrong-route", changed)
            .await
            .is_err()
    );
    let mut changed = request();
    changed.message.text = Some("different".into());
    assert_eq!(
        outbound::prepare(&f.state, &owner(), &f.channel, "fenced", changed.clone())
            .await
            .err()
            .unwrap()
            .status,
        409
    );
    assert_eq!(
        outbound::commit(
            &f.state,
            &owner(),
            &f.channel,
            &prepared.delivery_id,
            changed.message
        )
        .await
        .err()
        .unwrap()
        .status,
        409
    );
    f.change_record(&prepared.delivery_id, |v| {
        v["kind"] = "reply_delivery_v1".into()
    });
    assert_eq!(
        outbound::read(&f.state, &owner(), &f.channel, &prepared.delivery_id)
            .await
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    f.close().await;
}
#[tokio::test]
async fn outbound_cancel_send_race_has_one_winner_and_cancelled_never_sends() {
    let f = Fixture::new(Platform::Telegram).await;
    let prepared = f.prepare("cancelled").await;
    outbound::cancel(&f.state, &owner(), &f.channel, &prepared.delivery_id)
        .await
        .unwrap();
    assert_eq!(
        outbound::commit(
            &f.restart(),
            &owner(),
            &f.channel,
            &prepared.delivery_id,
            message()
        )
        .await
        .unwrap()
        .status,
        OutboundStatus::Cancelled
    );
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
    for n in 0..8 {
        let prepared = f.prepare(&format!("race-{n}")).await;
        let caller = owner();
        let (cancel, send) = tokio::join!(
            outbound::cancel(&f.state, &caller, &f.channel, &prepared.delivery_id),
            outbound::commit(
                &f.state,
                &caller,
                &f.channel,
                &prepared.delivery_id,
                message()
            )
        );
        let after = outbound::read(&f.state, &owner(), &f.channel, &prepared.delivery_id)
            .await
            .unwrap();
        if after.status == OutboundStatus::Cancelled {
            assert!(cancel.is_ok());
            assert_eq!(send.unwrap().status, OutboundStatus::Cancelled);
        } else {
            assert!(cancel.is_err());
            assert_eq!(send.unwrap().status, OutboundStatus::Committed);
        }
    }
    f.close().await;
}
#[tokio::test]
async fn outbound_lost_response_and_crash_after_commit_never_resend() {
    let f = Fixture::new(Platform::Telegram).await;
    f.probe.lost.store(true, Ordering::SeqCst);
    let one = f.prepare("lost").await;
    assert_eq!(
        outbound::commit(&f.state, &owner(), &f.channel, &one.delivery_id, message())
            .await
            .unwrap()
            .delivery
            .unwrap()
            .kind(),
        "unknown"
    );
    outbound::commit(
        &f.restart(),
        &owner(),
        &f.channel,
        &one.delivery_id,
        message(),
    )
    .await
    .unwrap();
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    f.probe.blocked.store(true, Ordering::SeqCst);
    let two = f.prepare("crash").await;
    let (state, channel, id) = (f.state.clone(), f.channel.clone(), two.delivery_id.clone());
    let entered = f.probe.entered.notified();
    // Clear the permit left by the first exchange before starting the second.
    entered.await;
    let worker =
        tokio::spawn(
            async move { outbound::commit(&state, &owner(), &channel, &id, message()).await },
        );
    f.probe.entered.notified().await;
    worker.abort();
    let _ = worker.await;
    let result = outbound::commit(
        &f.restart(),
        &owner(),
        &f.channel,
        &two.delivery_id,
        message(),
    )
    .await
    .unwrap();
    assert_eq!(result.delivery.unwrap().kind(), "unknown");
    assert!(
        outbound::cancel(&f.state, &owner(), &f.channel, &two.delivery_id)
            .await
            .is_err()
    );
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 2);
    f.close().await;
}
#[tokio::test]
async fn outbound_revoked_owner_credential_and_expiry_are_never_dispatched() {
    for mutation in ["owner", "credential", "route", "source", "expired"] {
        let f = Fixture::new(Platform::Telegram).await;
        let prepared = f.prepare(mutation).await;
        match mutation {
            "owner" => f.change_record(&f.record, |v| v["owner"]["subject"] = "bob".into()),
            "credential" => f.change_record(&f.record, |v| {
                v["secrets"]["channel_key"] = "rotated-fixture".into()
            }),
            "route" => f.change_record(&f.record, |v| v["sources"][0]["route_ids"] = json!([])),
            "source" => f.change_record(&f.record, |v| {
                v["sources"][0]["trust_normalized"] = true.into()
            }),
            _ => f.change_record(&prepared.delivery_id, |v| {
                v["receipt"]["expires_at"] = 1.into()
            }),
        }
        assert!(outbound::commit(
            &f.restart(),
            &owner(),
            &f.channel,
            &prepared.delivery_id,
            message()
        )
        .await
        .is_err());
        assert_eq!(f.probe.calls.load(Ordering::SeqCst), 0);
        f.close().await;
    }
}

#[path = "outbound_media.rs"]
mod media;

#[tokio::test]
async fn outbound_harmless_channel_rekey_does_not_revoke_reviewed_message() {
    let f = Fixture::new(Platform::Telegram).await;
    let prepared = f.prepare("reviewed-before-rekey").await;
    let id = cmaeg_core::ids::ChannelId::new(&f.channel).unwrap();
    let before = management::load(&f.state, &owner(), &id).await.unwrap();
    management::rekey(&f.state, &id).await.unwrap();
    let after = management::load(&f.state, &owner(), &id).await.unwrap();
    assert!(after.record.version > before.record.version);
    assert_eq!(
        serde_json::to_value(&before.definition.sources).unwrap(),
        serde_json::to_value(&after.definition.sources).unwrap()
    );
    let result = outbound::commit(
        &f.restart(),
        &owner(),
        &f.channel,
        &prepared.delivery_id,
        message(),
    )
    .await
    .unwrap();
    assert_eq!(result.delivery.unwrap().kind(), "confirmed");
    assert_eq!(f.probe.calls.load(Ordering::SeqCst), 1);
    f.close().await;
}
