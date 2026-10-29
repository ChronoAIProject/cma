//! Restart at the completed-parent/late-transcript boundary uses the active
//! media index and produces independent replies without replaying original work.
use super::super::*;
use crate::{
    delivery::{
        custom::CustomDelivery, durable::DeliveryCoordinator, router::ShardedScheduler,
        sync::SyncRegistry,
    },
    media::transcribe::{AudioContainer, AudioInput, TranscribeError, Transcriber, Transcript},
    turns::routed::{
        custom_http_media_tests::inline_media_with, media_delivery::MediaDelivery, test_support::*,
        DispatchOutcome, MediaUploadReceipt, ReplayEffect, RoutedDelivery,
    },
};
use axum::{
    body::to_bytes,
    extract::{Request, State},
    response::{IntoResponse, Response, Sse},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::seal::ReplyAddress;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tokio::sync::mpsc;

struct TranscriptService {
    unavailable: bool,
    calls: Arc<AtomicUsize>,
}
impl Transcriber for TranscriptService {
    fn enabled(&self) -> bool {
        true
    }
    fn accepts(&self, value: AudioContainer) -> bool {
        value == AudioContainer::Wav
    }
    fn transcribe<'a>(
        &'a self,
        _: AudioInput,
        _: tokio::time::Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.unavailable {
                return Err(TranscribeError::Failed);
            }
            Ok(Transcript {
                text: "Late transcript from exact original audio.".into(),
                usage_seconds: Some(1),
            })
        })
    }
}
type Requests = Arc<Mutex<Vec<Value>>>;
fn metadata(kind: &str) -> Value {
    json!({
        "cma_parent_command_id":"cmd_parent", "cma_parent_response_id":"resp_1",
        "cma_parent_fingerprint":"parent_fingerprint", "cma_artifact_id":format!("0:{kind}"),
        "cma_command_fingerprint":format!("fingerprint_{kind}")
    })
}
async fn media_manager(State(requests): State<Requests>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    if request.method() == axum::http::Method::POST && path.ends_with("/media/0/supplement") {
        let body: Value =
            serde_json::from_slice(&to_bytes(request.into_body(), 128 * 1024).await.unwrap())
                .unwrap();
        let kind = body["artifact"]["kind"].as_str().unwrap().to_owned();
        requests.lock().unwrap().push(body);
        return (
            axum::http::StatusCode::ACCEPTED,
            Json(json!({
                "command_id":format!("cmd_{kind}"), "response_id":format!("resp_{kind}"),
                "response_url":format!("/api/v2/responses/resp_{kind}"), "status":"queued",
                "queue_deadline":"2099-01-01T00:00:00Z", "metadata":metadata(&kind)
            })),
        )
            .into_response();
    }
    if request.method() == axum::http::Method::GET && path.starts_with("/api/v2/responses/resp_") {
        let kind = path.strip_prefix("/api/v2/responses/resp_").unwrap();
        return Json(json!({
            "id":format!("resp_{kind}"), "object":"response", "status":"completed",
            "conversation":{"id":"agt_existing"}, "metadata":metadata(kind),
            "output":[{"type":"message","content":[{"type":"output_text","text":format!("Processed {kind}.")}]}]
        })).into_response();
    }
    panic!("unexpected provider request: {} {path}", request.method());
}
#[tokio::test]
async fn media_worker_restart_after_original_completion_delivers_late_transcript_without_original_replay(
) {
    let waiters = Arc::new(SyncRegistry::default());
    let (waiter_id, waiter) = waiters.register().unwrap();
    let custom = CustomDelivery::new("https://gateway.example".into(), waiters);
    let original_bytes = b"RIFF\x10\x00\x00\x00WAVEfmt \x00\x00\x00\x00";
    let mut activity = fixture::custom(None);
    let cmaeg_core::activity::ActivityKind::Event {
        attachments, ce, ..
    } = &mut activity.kind
    else {
        unreachable!()
    };
    ce.data =
        Some(json!({"audio":format!("data:audio/wav;base64,{}", STANDARD.encode(original_bytes))}));
    attachments.push(serde_json::from_value(json!({"reference":"audio","kind":"audio","filename":"audio.wav","source":{"type":"inline","pointer":"/audio"}})).unwrap());
    let mut request = fixture::request(
        |v| {
            v["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_existing"});
            v["reply"]["sinks"] = json!([{"type":"sync"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Sync { waiter: waiter_id }
    });
    let setup = Setup::with_request(supported(), request).await;
    let index_server = crate::ingress::routing_store::test_server::Server::start().await;
    let index = MediaWorkIndex::fixture(
        setup.journal().clone(),
        setup.server.store(),
        index_server.records.clone(),
    );
    let mut prepared = setup
        .journal()
        .claim(&setup.plan, setup.target(), "prepare-media")
        .await
        .unwrap();
    let mut body = frozen(
        &setup.plan,
        setup.target(),
        &setup.current.channel.lock().unwrap(),
        128,
    );
    body.metadata.insert(
        super::super::super::media_schedule::KEY.into(),
        r#"{"v":1,"t":[0],"x":[]}"#.into(),
    );
    setup
        .journal()
        .freeze(&mut prepared, &setup.plan, setup.target(), &body)
        .await
        .unwrap();
    index
        .prepare(&setup.plan, setup.target(), &body)
        .await
        .unwrap();
    setup.journal().release(&mut prepared).await.unwrap();
    let DispatchOutcome::Replay { mut lease, .. } = setup.dispatch().await.unwrap() else {
        panic!("accepted");
    };
    let (tx, _rx) = mpsc::channel(1);
    let coordinator =
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))).with_custom_stream(custom);
    let original_delivery = RoutedDelivery::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        coordinator.clone(),
    );
    original_delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &recovery_frame("resp_1", 0, true),
            ReplayEffect::NoEffect,
        )
        .await
        .unwrap();
    setup
        .journal()
        .complete(&mut lease, "resp_1")
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    let parent_before = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(parent_before.0.phase, Phase::Completed { .. }));

    let store = MediaObligationStore(setup.journal().clone());
    store
        .initialize(
            &setup.plan,
            setup.target(),
            0,
            None,
            Some(MediaContinuationKind::Transcript),
        )
        .await
        .unwrap();
    let mut media_lease = store
        .claim(&setup.plan, setup.target(), 0, "persist-upload")
        .await
        .unwrap();
    store
        .freeze_original(
            &mut media_lease,
            MediaArtifactClaim {
                attachment_index: 0,
                kind: MediaContinuationKind::Original,
                content_sha256: sha256(original_bytes),
                source_sha256: sha256(original_bytes),
                name: "audio.wav".into(),
                media_type: "audio/x-wav".into(),
                size_bytes: original_bytes.len() as u64,
                text_sha256: None,
            },
        )
        .await
        .unwrap();
    store
        .record_upload(
            &mut media_lease,
            MediaUploadReceipt {
                id: "att_audio".into(),
                agent_id: "agt_existing".into(),
                name: "audio.wav".into(),
                path: "/home/attachments/agt_existing/att_audio/audio.wav".into(),
                size_bytes: original_bytes.len() as u64,
                media_type: "audio/x-wav".into(),
                sha256: sha256(original_bytes),
            },
        )
        .await
        .unwrap();
    store.release(&mut media_lease).await.unwrap();
    let requests: Requests = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .fallback(media_manager)
        .with_state(requests.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let registry = crate::providers::ProviderRegistry::parse(&json!([{
        "slug":"cma_codex","base_url":base,"kind":"responses_http","max_inflight":2,
        "paths":{"agent_card":"/card","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses", "recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}
    }]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 2)
            .unwrap()
            .with_cma_origin_fixture(&base),
    );
    let delivery = Arc::new(MediaDelivery {
        store: store.clone(),
        authority: setup.current.clone(),
        providers: providers.clone(),
        coordinator,
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let worker = |unavailable| MediaWorker {
        index: index.clone(),
        plans: setup.server.store(),
        store: store.clone(),
        authority: setup.current.clone(),
        providers: providers.clone(),
        media: inline_media_with(Arc::new(TranscriptService {
            unavailable,
            calls: calls.clone(),
        })),
        delivery: delivery.clone(),
        worker: format!("media-restart-{unavailable}"),
    };
    let work = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.target().id.clone(),
    };
    let unavailable = worker(true).run(&work).await;
    assert!(
        matches!(unavailable, Err(ExecutionError::Unavailable)),
        "{unavailable:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the first transcript attempt reached the unavailable transcriber"
    );
    let pending = store
        .load(&setup.plan, setup.target(), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.supplements.len(), 1);
    assert!(
        pending.supplements[0].settled,
        "original-file reply must survive unavailable transcription"
    );
    let pending_page = store.retained_responses(&setup.plan, 0).await.unwrap();
    assert!(pending_page
        .items
        .iter()
        .any(|item| item.kind == "transcript"
            && item.status == "pending"
            && item.response_id.is_none()));
    assert!(index
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .is_some());
    assert!(worker(false).run(&work).await.unwrap());
    assert!(index
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .is_none());
    let settled = store
        .load(&setup.plan, setup.target(), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.supplements.len(), 2);
    assert!(settled.supplements.iter().all(|s| s.settled));
    {
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "only one append per immutable artifact");
        assert_eq!(requests[1]["artifact"]["kind"], "transcript");
        assert_eq!(
            requests[1]["artifact"]["source_sha256"],
            sha256(original_bytes)
        );
        assert_eq!(
            requests[1]["derived_text"],
            "Late transcript from exact original audio."
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let parent_after = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent_before.1, parent_after.1);
    assert_eq!(
        super::super::super::storage::bytes(&parent_before.0).unwrap(),
        super::super::super::storage::bytes(&parent_after.0).unwrap()
    );
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    let response = Sse::new(waiter.stream()).into_response();
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        to_bytes(response.into_body(), 128 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(
        stream.matches("event: cmaeg.media.response").count(),
        2,
        "{stream}"
    );
    assert_eq!(
        stream.matches("event: cmaeg.completed").count(),
        1,
        "{stream}"
    );
    assert!(stream.contains("Processed original.") && stream.contains("Processed transcript."));
    server.abort();
}
