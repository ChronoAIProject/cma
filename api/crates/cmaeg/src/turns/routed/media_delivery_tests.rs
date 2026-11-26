#[path = "media_delivery_authority_tests.rs"]
mod authority_tests;
use super::{
    adapter_test_server::HttpServer, delivery_tests::accepted, media_delivery::MediaDelivery,
    media_obligations::*, media_response_store::NativeOutcome, media_worker::MediaResponseDelivery,
    test_support::*,
};
use crate::delivery::{
    custom::CustomDelivery,
    durable::DeliveryCoordinator,
    local_stream::{CustomStreamEffect, LocalStreamOutcome},
    router::ShardedScheduler,
    sync::{SyncRegistry, Waiter},
};
use axum::response::{IntoResponse, Sse};
use cmaeg_core::{
    media_continuation::{sha256, MediaArtifactClaim, MediaContinuationKind},
    seal::ReplyAddress,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
struct Fixture {
    setup: Setup,
    store: MediaObligationStore,
    lease: MediaLease,
    http: HttpServer,
    delivery: MediaDelivery,
    custom: CustomDelivery,
    waiter: Option<Waiter>,
    _rx: mpsc::Receiver<crate::delivery::intents::DeliveryCommand>,
}
impl Fixture {
    async fn new() -> Self {
        Self::kind(true).await
    }
    async fn kind(custom: bool) -> Self {
        Self::configured(custom, true).await
    }
    async fn configured(custom: bool, auto_final: bool) -> Self {
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = waiters.register_with_media_responses(true).unwrap();
        let mut activity = if custom {
            fixture::custom(None)
        } else {
            fixture::message("Use the attachment")
        };
        let attachments = match &mut activity.kind {
            cmaeg_core::activity::ActivityKind::Event { attachments, .. }
            | cmaeg_core::activity::ActivityKind::Message { attachments, .. } => attachments,
            _ => unreachable!(),
        };
        let source = if custom {
            json!({"type":"inline","pointer":"/file"})
        } else {
            json!({"type":"telegram","file_id":"file_fixture","file_unique_id":"unique_fixture"})
        };
        attachments.push(
            serde_json::from_value(
                json!({"reference":"file","kind":"file","filename":"file.txt","source":source}),
            )
            .unwrap(),
        );
        let mut request = fixture::request(
            |v| {
                v["provider"]["target"] =
                    json!({"type":"existing_agent","agent_id":"agt_existing"});
                if custom {
                    v["reply"]["sinks"] = json!([{"type":"sync"}]);
                }
            },
            activity,
        );
        if custom {
            fixture::rewrite_ref(&mut request, |event| {
                event.reply = ReplyAddress::Sync { waiter: id }
            });
        }
        // Authoring requires auto_final for newly configured existing Agents.
        // The delivery reader still honors a retained disabled reply policy.
        Arc::make_mut(&mut request.channel)
            .definition
            .reply
            .auto_final = auto_final;
        let setup = Setup::with_request(supported(), request).await;
        let original = accepted(&setup).await;
        let store = MediaObligationStore(setup.journal().clone());
        store
            .initialize(&setup.plan, setup.target(), 0, None, None)
            .await
            .unwrap();
        let mut lease = store
            .claim(&setup.plan, setup.target(), 0, "media-test")
            .await
            .unwrap();
        let artifact = MediaArtifactClaim {
            attachment_index: 0,
            kind: MediaContinuationKind::Original,
            content_sha256: sha256(b"file"),
            source_sha256: sha256(b"file"),
            name: "file.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: 4,
            text_sha256: None,
        };
        store
            .freeze_original(&mut lease, artifact.clone())
            .await
            .unwrap();
        store
            .record_upload(
                &mut lease,
                MediaUploadReceipt {
                    id: "att_fixture".into(),
                    agent_id: "agt_existing".into(),
                    name: "file.txt".into(),
                    path: "/home/attachments/agt_existing/att_fixture/file.txt".into(),
                    size_bytes: 4,
                    media_type: "text/plain".into(),
                    sha256: sha256(b"file"),
                },
            )
            .await
            .unwrap();
        store
            .freeze_supplement(&mut lease, artifact, "")
            .await
            .unwrap();
        let receipt = MediaContinuationReceipt {
            command_id: "cmd_media".into(),
            response_id: "resp_accepted".into(),
            metadata: [
                ("cma_parent_command_id", "cmd_parent"),
                ("cma_parent_response_id", "resp_1"),
                ("cma_parent_fingerprint", "parent_fingerprint"),
                ("cma_artifact_id", "0:original"),
                ("cma_command_fingerprint", "command_fingerprint"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        };
        store
            .record_submission(&mut lease, MediaContinuationKind::Original, receipt.clone())
            .await
            .unwrap();
        let http = HttpServer::start().await;
        http.memory.lock().unwrap().config.snapshot_body = Some(
            json!({"id":"resp_accepted","object":"response","status":"completed","conversation":{"id":"agt_existing"},"metadata":receipt.metadata,"output":[{"type":"message","content":[{"type":"output_text","text":"Processed the persisted attachment."}]}]}),
        );
        let custom = CustomDelivery::new("https://gateway.example".into(), waiters);
        let (tx, rx) = mpsc::channel(1);
        let delivery = MediaDelivery {
            store: store.clone(),
            authority: setup.current.clone(),
            providers: http.client(true, Duration::from_secs(2)),
            coordinator: DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx)))
                .with_custom_stream(custom.clone()),
        };
        setup.journal().guard(&original).await.unwrap();
        Self {
            setup,
            store,
            lease,
            http,
            delivery,
            custom,
            waiter: Some(waiter),
            _rx: rx,
        }
    }
    async fn stream(&mut self) -> String {
        let response = Sse::new(self.waiter.take().unwrap().stream()).into_response();
        let bytes = tokio::time::timeout(
            Duration::from_secs(2),
            axum::body::to_bytes(response.into_body(), 128 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }
}
#[tokio::test]
async fn media_response_disabled_automatic_text_preserves_terminal_custom_control() {
    let mut f = Fixture::configured(true, false).await;
    f.delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease)
        .await
        .unwrap();
    f.store.release(&mut f.lease).await.unwrap();
    f.delivery
        .finish(&f.setup.plan, f.setup.target())
        .await
        .unwrap();
    let text = f.stream().await;
    assert!(text.contains("event: cmaeg.media.response"), "{text}");
    assert!(text.contains("resp_accepted"), "{text}");
    assert!(
        !text.contains("Processed the persisted attachment."),
        "{text}"
    );
    assert!(text.contains("event: cmaeg.completed"), "{text}");
}
#[tokio::test]
async fn media_response_reaches_original_custom_stream_with_distinct_durable_identity_and_no_original_replay(
) {
    let mut f = Fixture::new().await;
    let event = super::model::delivery_event(&f.setup.plan, f.setup.target());
    assert_eq!(
        f.custom.local_response(
            &event,
            &CustomStreamEffect::Completed {
                final_text: Some("Original complete".into())
            },
            "resp_1"
        ),
        LocalStreamOutcome::Buffered
    );
    f.delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease)
        .await
        .unwrap();
    assert!(f.lease.state().supplements[0].settled);
    let state = f
        .store
        .media_response(&f.lease, MediaContinuationKind::Original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.response_id, "resp_accepted");
    assert_eq!(
        f.store.media_response_text(&f.lease, &state).await.unwrap(),
        "Processed the persisted attachment."
    );
    f.store.release(&mut f.lease).await.unwrap();
    f.delivery
        .finish(&f.setup.plan, f.setup.target())
        .await
        .unwrap();
    let text = f.stream().await;
    assert_eq!(
        text.matches("event: cmaeg.response.completed").count(),
        1,
        "{text}"
    );
    assert_eq!(
        text.matches("event: cmaeg.media.response").count(),
        1,
        "{text}"
    );
    assert_eq!(text.matches("event: cmaeg.completed").count(), 1, "{text}");
    let terminal: serde_json::Value = serde_json::from_str(
        text.split("event: cmaeg.completed\n")
            .nth(1)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(terminal["text"], "Original complete");
    assert_eq!(terminal["message"], serde_json::Value::Null);
    assert_eq!(terminal["error"], serde_json::Value::Null);
    assert_eq!(terminal["media_complete"], true);
    assert!(
        text.contains("Processed the persisted attachment.")
            && text.contains("resp_accepted")
            && text.contains("parent_response_id")
    );
    assert_eq!(f.setup.port.starts.lock().unwrap().len(), 1);
    assert!(f.http.seen().iter().all(|r| r.method == "GET"));
}
#[tokio::test]
async fn media_response_wrong_agent_or_metadata_cannot_reach_custom_sink() {
    let mut f = Fixture::new().await;
    f.http
        .memory
        .lock()
        .unwrap()
        .config
        .snapshot_body
        .as_mut()
        .unwrap()["conversation"]["id"] = "agt_foreign".into();
    assert!(f
        .delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease)
        .await
        .is_err());
    assert!(f
        .store
        .media_response(&f.lease, MediaContinuationKind::Original)
        .await
        .unwrap()
        .is_none());
    {
        let mut memory = f.http.memory.lock().unwrap();
        let body = memory.config.snapshot_body.as_mut().unwrap();
        body["conversation"]["id"] = "agt_existing".into();
        body["metadata"]["cma_artifact_id"] = "1:original".into();
    }
    assert!(f
        .delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease)
        .await
        .is_err());
    assert!(!f.lease.state().supplements[0].settled);
}
#[tokio::test]
async fn media_response_unknown_local_intent_is_retained_and_not_resent_after_restart() {
    let mut f = Fixture::new().await;
    let (parent, _) = f
        .store
        .0
        .read::<super::TargetState>(&f.setup.target().id)
        .await
        .unwrap()
        .unwrap();
    let body = f.store.0.retained_body(&parent).await.unwrap();
    let event = super::model::delivery_event(&f.setup.plan, f.setup.target());
    let state = f
        .store
        .freeze_media_response(
            &f.lease,
            MediaContinuationKind::Original,
            "completed",
            "Processed the persisted attachment.",
            Some(super::CustomCheckpoint::new(
                &event,
                &body.delivery.policy.sinks,
                true,
            )),
        )
        .await
        .unwrap();
    assert!(matches!(state.native, NativeOutcome::NotStarted));
    assert_eq!(super::media_delivery::summary(&state), "pending");
    f.store.release(&mut f.lease).await.unwrap();
    f.lease = f
        .store
        .claim(&f.setup.plan, f.setup.target(), 0, "replacement")
        .await
        .unwrap();
    f.delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease)
        .await
        .unwrap();
    let state = f
        .store
        .media_response(&f.lease, MediaContinuationKind::Original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.custom.unwrap().local, Some(LocalStreamOutcome::Lost));
    assert!(
        f.http.seen().is_empty(),
        "persisted response requires no repeat snapshot"
    );
    f.store.release(&mut f.lease).await.unwrap();
    f.delivery
        .finish(&f.setup.plan, f.setup.target())
        .await
        .unwrap();
    let text = f.stream().await;
    assert!(text.contains("cmaeg.failed"));
    assert!(!text.contains("cmaeg.media.response"));
}

#[tokio::test]
async fn media_response_reaches_exact_native_destination_with_independent_confirmed_receipt() {
    use crate::delivery::{
        durable::receipt_for,
        intents::{DeliveryCommand, DeliveryOutcome},
    };
    let mut f = Fixture::kind(false).await;
    let expected = super::model::delivery_event(&f.setup.plan, f.setup.target());
    let send = f
        .delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease);
    let receive = async {
        let DeliveryCommand::DurableReply(reply) = f._rx.recv().await.unwrap() else {
            panic!("native reply");
        };
        assert_eq!(reply.event, expected);
        assert_eq!(
            reply.message.text.as_deref(),
            Some("Processed the persisted attachment.")
        );
        let _ticket = reply.gate.enter_route("fixture-native").await.unwrap();
        let _attempt = reply.gate.attempt().await.unwrap();
        assert!(reply.gate.commit());
        reply
            .receipt
            .send(receipt_for(DeliveryOutcome::Confirmed {
                platform_message_id: Some("native-media-response".into()),
            }))
            .unwrap();
    };
    let (result, _) = tokio::join!(send, receive);
    result.unwrap();
    let state = f
        .store
        .media_response(&f.lease, MediaContinuationKind::Original)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(state.native,NativeOutcome::Settled{outcome:DeliveryOutcome::Confirmed{platform_message_id:Some(ref id)}} if id=="native-media-response")
    );
    assert!(f.lease.state().supplements[0].settled);
    assert_eq!(f.setup.port.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn retained_media_response_metadata_survives_event_expiry_without_text_or_workspace_disclosure(
) {
    let mut f = Fixture::new().await;
    f.delivery
        .deliver(&f.setup.plan, f.setup.target(), &mut f.lease)
        .await
        .unwrap();
    f.setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 901, std::sync::atomic::Ordering::SeqCst);
    let page = f.store.retained_responses(&f.setup.plan, 0).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].response_id.as_deref(), Some("resp_accepted"));
    assert_eq!(
        page.items[0].response_url.as_deref(),
        Some("/api/v2/responses/resp_accepted")
    );
    assert_eq!(page.items[0].status, "completed");
    let wire = serde_json::to_string(&page).unwrap();
    for private in [
        "Processed the persisted attachment.",
        "/home/attachments/",
        "cmd_media",
        "parent_fingerprint",
    ] {
        assert!(!wire.contains(private));
    }
}
