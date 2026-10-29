//! Join real eager preparation and schedule capture to delayed Custom delivery.
use super::*;
use crate::{
    media::prepare::PrepareConfig,
    turns::routed::{FrozenTurnBuilder, ProductionFrozenTurnBuilder},
};
use tokio::sync::{Notify, Semaphore};

const AUDIO: &[u8] = b"RIFF\x10\x00\x00\x00WAVEfmt \x00\x00\x00\x00";
const WORDS: &str = "Transcript released only after the original response completed.";

struct HeldTranscript {
    calls: AtomicUsize,
    entered: Notify,
    release: Semaphore,
}
impl Transcriber for HeldTranscript {
    fn enabled(&self) -> bool {
        true
    }
    fn accepts(&self, container: AudioContainer) -> bool {
        container == AudioContainer::Wav
    }
    fn transcribe<'a>(
        &'a self,
        _: AudioInput,
        _: tokio::time::Instant,
    ) -> BoxFuture<'a, Result<Transcript, TranscribeError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            Ok(Transcript {
                text: WORDS.into(),
                usage_seconds: Some(1),
            })
        })
    }
}

#[derive(Clone, Default)]
struct Manager {
    supplements: Requests,
    paths: Arc<Mutex<Vec<(String, String)>>>,
}
async fn manager(State(state): State<Manager>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    state
        .paths
        .lock()
        .unwrap()
        .push((request.method().to_string(), path.clone()));
    if request.method() == axum::http::Method::PUT {
        assert!(path.contains("/conversations/") && !path.contains("/events/"));
        return Json(json!({})).into_response();
    }
    if request.method() == axum::http::Method::POST && path.ends_with("/media/0/attachment") {
        let bytes = to_bytes(request.into_body(), 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), AUDIO);
        return Json(json!({
            "id":"att_audio", "agent_id":"agt_existing", "name":"audio.wav",
            "path":"/home/attachments/agt_existing/att_audio/audio.wav",
            "size_bytes":AUDIO.len(), "media_type":"audio/x-wav", "sha256":sha256(AUDIO)
        }))
        .into_response();
    }
    media_manager(State(state.supplements), request).await
}

#[tokio::test]
async fn real_pending_preparation_delivers_late_transcript_to_same_agent_and_custom_stream() {
    let waiters = Arc::new(SyncRegistry::default());
    let (waiter_id, waiter) = waiters.register_with_media_responses(true).unwrap();
    let custom = CustomDelivery::new("https://gateway.example".into(), waiters);
    let mut activity = fixture::custom(None);
    let cmaeg_core::activity::ActivityKind::Event {
        attachments, ce, ..
    } = &mut activity.kind
    else {
        unreachable!()
    };
    ce.data = Some(json!({"audio":format!("data:audio/wav;base64,{}", STANDARD.encode(AUDIO))}));
    attachments.push(
        serde_json::from_value(json!({
            "reference":"audio", "kind":"audio", "filename":"audio.wav", "duration_seconds":1,
            "source":{"type":"inline","pointer":"/audio"}
        }))
        .unwrap(),
    );
    let mut request = fixture::request(
        |value| {
            value["provider"]["target"] =
                json!({"type":"existing_agent","agent_id":"agt_existing"});
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Sync { waiter: waiter_id }
    });
    let setup = Setup::with_request(supported(), request).await;
    let state = Manager::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().fallback(manager).with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let registry = crate::providers::ProviderRegistry::parse(&json!([{
        "slug":"cma_codex","base_url":base,"kind":"responses_http","max_inflight":2,
        "paths":{"agent_card":"/card","binding":"/api/v1/cmaeg/bindings/{binding_id}",
            "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
            "responses":"/api/v2/responses", "recovery":{
                "response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}
    }]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 2)
            .unwrap()
            .with_cma_origin_fixture(&base),
    );
    let speech = Arc::new(HeldTranscript {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    let media = inline_media_with(speech.clone());
    // Only this fixture's wait is short; no production budget or clock changes.
    // A barrier, rather than a race against a fixed sleep, holds the actual job.
    let builder = ProductionFrozenTurnBuilder::new(providers.clone(), Arc::new(fixture::keys()))
        .with_media(
            Some(media.clone()),
            PrepareConfig {
                budget: Duration::from_millis(100),
                ..PrepareConfig::default()
            },
        )
        .with_media_continuations(true);
    let current = setup.current.channel.lock().unwrap().clone();
    let body = builder
        .prepare(&setup.plan, setup.target(), &current)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), speech.entered.notified())
        .await
        .unwrap();
    let schedule: Value =
        serde_json::from_str(&body.metadata[crate::turns::routed::media_schedule::KEY]).unwrap();
    assert_eq!(schedule, json!({"v":1,"t":[0],"x":[]}));
    let input = serde_json::to_string(&body.input).unwrap();
    assert!(input.contains("transcript is still being prepared"));
    assert!(!input.contains(WORDS));
    assert_eq!(speech.calls.load(Ordering::SeqCst), 1);
    let original_body = body.provider_body();
    let index_server = crate::ingress::routing_store::test_server::Server::start().await;
    let index = MediaWorkIndex::fixture(
        setup.journal().clone(),
        setup.server.store(),
        index_server.records.clone(),
    );
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "prepare-real-media")
        .await
        .unwrap();
    setup
        .journal()
        .freeze(&mut lease, &setup.plan, setup.target(), &body)
        .await
        .unwrap();
    index
        .prepare(&setup.plan, setup.target(), &body)
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    let DispatchOutcome::Replay {
        mut lease, body, ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("original response must be accepted once");
    };
    assert_eq!(body.provider_body(), original_body);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
    let (tx, _rx) = mpsc::channel(1);
    let coordinator =
        DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))).with_custom_stream(custom);
    RoutedDelivery::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        coordinator.clone(),
    )
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
    let worker = MediaWorker {
        index: index.clone(),
        plans: setup.server.store(),
        store: store.clone(),
        authority: setup.current.clone(),
        providers: providers.clone(),
        media,
        delivery: Arc::new(MediaDelivery {
            store: store.clone(),
            authority: setup.current.clone(),
            providers,
            coordinator,
        }),
        worker: "real-preparation-continuation".into(),
    };
    let work = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.target().id.clone(),
    };
    assert!(!worker.run(&work).await.unwrap());
    let pending = store
        .load(&setup.plan, setup.target(), 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pending.pending_derivative,
        Some(MediaContinuationKind::Transcript)
    );
    assert_eq!(pending.supplements.len(), 1);
    assert!(
        pending.supplements[0].settled,
        "original-file reply must not wait for STT"
    );
    assert_eq!(pending.upload.as_ref().unwrap().agent_id, "agt_existing");
    assert!(store
        .retained_responses(&setup.plan, 0)
        .await
        .unwrap()
        .items
        .iter()
        .any(|row| row.kind == "transcript"
            && row.status == "pending"
            && row.response_id.is_none()));
    assert!(index
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .is_some());
    speech.release.add_permits(1);
    assert!(worker.run(&work).await.unwrap());
    assert_eq!(
        speech.calls.load(Ordering::SeqCst),
        1,
        "reuse the original background derivation"
    );
    assert!(index
        .load(&setup.plan, setup.target())
        .await
        .unwrap()
        .is_none());
    let requests = state.supplements.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["artifact"]["kind"], "original");
    assert_eq!(requests[1]["artifact"]["kind"], "transcript");
    assert_eq!(requests[1]["artifact"]["source_sha256"], sha256(AUDIO));
    assert_eq!(requests[1]["derived_text"], WORDS);
    let parent_after = setup
        .journal()
        .read::<TargetState>(&setup.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(parent_before.1, parent_after.1);
    assert_eq!(
        crate::turns::routed::storage::bytes(&parent_before.0).unwrap(),
        crate::turns::routed::storage::bytes(&parent_after.0).unwrap()
    );
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    let paths = state.paths.lock().unwrap().clone();
    let event_path = format!(
        "/api/v1/cmaeg/bindings/{}/conversations/{}/events/{}/media/0/",
        setup.target().launch.provider.binding_id,
        setup.target().launch.event.conversation_id,
        setup.target().launch.event.event_id
    );
    let upload_path = format!("{event_path}attachment");
    let supplement_path = format!("{event_path}supplement");
    assert_eq!(
        paths
            .iter()
            .filter(|(method, path)| method == "POST" && path == &upload_path)
            .count(),
        1
    );
    assert_eq!(
        paths
            .iter()
            .filter(|(method, path)| method == "POST" && path == &supplement_path)
            .count(),
        2
    );
    assert!(worker.run(&work).await.unwrap());
    assert_eq!(
        *state.paths.lock().unwrap(),
        paths,
        "settled worker must not send again"
    );
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        to_bytes(
            Sse::new(waiter.stream()).into_response().into_body(),
            128 * 1024,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let stream = String::from_utf8(bytes.to_vec()).unwrap();
    let frames: Vec<(&str, Value)> = stream
        .split("\n\n")
        .filter_map(|frame| {
            let event = frame
                .lines()
                .find_map(|line| line.strip_prefix("event: "))?;
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            Some((event, serde_json::from_str(data).unwrap()))
        })
        .collect();
    let supplements: Vec<_> = frames
        .iter()
        .filter(|(event, _)| *event == "cmaeg.media.response")
        .collect();
    assert_eq!(supplements.len(), 2, "{stream}");
    for (frame, kind) in supplements.iter().zip(["original", "transcript"]) {
        assert_eq!(frame.1["response_id"], format!("resp_{kind}"));
        assert_eq!(frame.1["parent_response_id"], "resp_1");
        assert_eq!(
            frame.1["conversation_id"],
            setup.target().launch.event.conversation_id.as_str()
        );
        assert_eq!(frame.1["kind"], kind);
        assert_eq!(frame.1["status"], "completed");
        assert_eq!(frame.1["text"], format!("Processed {kind}."));
    }
    assert_eq!(
        frames
            .iter()
            .filter(|(event, _)| *event == "cmaeg.completed")
            .count(),
        1
    );
    let final_frame = frames.last().unwrap();
    assert_eq!(final_frame.0, "cmaeg.completed");
    assert_eq!(final_frame.1["media_complete"], true);
    assert_eq!(final_frame.1["status"], "completed");
    server.abort();
}
