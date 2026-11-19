use super::*;
use cmaeg_core::activity::{ActorKind, SourceKind};
use serde_json::{json, Value};

const ACTOR_ID: &str = "ou_synthetic_sender_sentinel";
const ACTOR_NAME: &str = "Synthetic private display name";

pub(super) fn private_actor(request: &mut LaunchRequest) {
    request.activity.actor.id = ACTOR_ID.into();
    request.activity.actor.display_name = Some(ACTOR_NAME.into());
    request.activity.actor.kind = ActorKind::Human;
    request.activity.source = SourceKind::NativeLark(cmaeg_core::channel::NativeLarkIdentity {
        region: cmaeg_core::channel::LarkRegion::Lark,
        app_id: "cli_fixture".into(),
        tenant_key: "tenant_fixture".into(),
        bot_open_id: "ou_fixture_bot".into(),
    });
    let digest = cmaeg_core::seal::event_ref::activity_digest(&request.activity);
    fixture::rewrite_ref(request, |event| event.activity_sha256 = Some(digest));
}

fn assert_public(bytes: &[u8], canonical: &ActivityPage, label: &str) {
    let body = std::str::from_utf8(bytes).unwrap();
    assert!(!body.contains(ACTOR_ID), "raw actor ID in response");
    assert!(!body.contains(ACTOR_NAME), "upstream name in response");
    let public: ActivityPage = serde_json::from_slice(bytes).unwrap();
    let mut expected = serde_json::to_value(canonical).unwrap();
    for item in expected["items"].as_array_mut().unwrap() {
        item["actor"]["id"] = json!("redacted");
        item["actor"]["display_name"] = json!(label);
    }
    assert_eq!(serde_json::to_value(public).unwrap(), expected);
}

fn assert_frame(bytes: &[u8], canonical: &ActivityPage, event: &str) {
    let frame = std::str::from_utf8(bytes).unwrap();
    assert!(frame.contains(&format!("event: {event}\n")));
    assert!(frame.contains(&format!("id: {}\n", canonical.cursor)));
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    assert_public(data.as_bytes(), canonical, "Participant");
}

fn stored_rows(server: &Server) -> Value {
    json!(server
        .memory
        .rows
        .lock()
        .unwrap()
        .iter()
        .map(|(id, row)| { json!([id, row.owner, row.blob, row.expiry, row.version]) })
        .collect::<Vec<_>>())
}

#[tokio::test]
async fn retained_actor_is_private_on_rest_and_every_stream_snapshot_without_rewriting_receipts() {
    let (server, mut state, mut request, mut creator) = fixture_app().await;
    private_actor(&mut request);
    let _auth = live_auth(&mut state, &mut creator).await;
    let record = request.channel.definition.record_id.as_str();
    state.activity.received(&request).await.unwrap();
    // Recreate the reader over a receipt persisted with its original actor.
    state.activity = Arc::new(ActivityStore::new(
        server.records.clone(),
        Arc::new(fixture::keys()),
    ));
    let canonical = state
        .activity
        .latest(&creator.subject, record)
        .await
        .unwrap();
    assert_eq!(canonical.items[0].actor, request.activity.actor);
    let original = stored_rows(&server);
    let projected = crate::activity::public_page(&canonical);
    assert_eq!(
        serde_json::to_value(crate::activity::public_page(&projected)).unwrap(),
        serde_json::to_value(&projected).unwrap()
    );
    for _ in 0..2 {
        let response = crate::http::activity::latest(
            State(state.clone()),
            creator.clone(),
            Path(request.channel.id.to_string()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        assert_public(&body, &canonical, "Participant");
    }
    let mut stale: Cursor =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&canonical.cursor).unwrap()).unwrap();
    stale.day -= 8;
    let stale = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&stale).unwrap());
    for (cursor, header, event) in [
        (None, None, "activity"),
        (Some(canonical.cursor.clone()), None, "activity"),
        (Some(stale.clone()), None, "reset"),
        (None, Some(stale), "reset"),
        (None, Some(canonical.cursor.clone()), "activity"),
    ] {
        let mut headers = auth_headers();
        if let Some(header) = header {
            headers.insert("last-event-id", header.parse().unwrap());
        }
        let response = crate::http::activity::stream(
            State(state.clone()),
            creator.clone(),
            Path(request.channel.id.to_string()),
            Query(crate::http::activity::ActivityQuery { cursor }),
            headers,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let mut stream = response.into_body().into_data_stream();
        assert_frame(&stream.next().await.unwrap().unwrap(), &canonical, event);
    }
    assert_eq!(stored_rows(&server), original);
    let response = crate::http::activity::stream(
        State(state.clone()),
        creator.clone(),
        Path(request.channel.id.to_string()),
        Query(Default::default()),
        auth_headers(),
    )
    .await;
    let mut stream = response.into_body().into_data_stream();
    assert_frame(
        &stream.next().await.unwrap().unwrap(),
        &canonical,
        "activity",
    );
    state
        .activity
        .change(
            record,
            request.activity.event_id.as_str(),
            &Change::state(
                "default",
                ConsumerState::Produced,
                Some("response-private-fixture".into()),
            ),
        )
        .await
        .unwrap();
    let updated = state
        .activity
        .latest(&creator.subject, record)
        .await
        .unwrap();
    let after_write = stored_rows(&server);
    let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_frame(&frame, &updated, "activity");
    // Overflow the notification buffer to exercise the lagged reset path.
    for _ in 0..300 {
        state.activity.updates.send(record.to_owned()).unwrap();
    }
    let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_frame(&frame, &updated, "reset");
    assert_eq!(stored_rows(&server), after_write);
    assert_eq!(updated.items[0].actor, request.activity.actor);
    state.activity.received(&request).await.unwrap();
    assert_eq!(
        stored_rows(&server),
        after_write,
        "duplicate receipt changed after public reads"
    );
}

#[tokio::test]
async fn every_actor_kind_and_upstream_name_has_a_fixed_public_label() {
    let (_server, state, mut request, creator) = fixture_app().await;
    private_actor(&mut request);
    let record = request.channel.definition.record_id.as_str();
    state.activity.received(&request).await.unwrap();
    let names = [
        None,
        Some(String::new()),
        Some(" \t ".into()),
        Some("合成表示名".into()),
        Some("x".repeat(1024)),
        Some("<b>sender</b>".into()),
        Some(ACTOR_ID.into()),
    ];
    for (kind, label) in [
        (ActorKind::Human, "Participant"),
        (ActorKind::Bot, "Bot"),
        (ActorKind::System, "System"),
        (ActorKind::Unknown, "Actor not reported"),
    ] {
        for name in &names {
            state
                .activity
                .modify(&creator.subject, record, today(), |bucket| {
                    bucket.items[0].actor.kind = kind;
                    bucket.items[0].actor.display_name = name.clone();
                })
                .await
                .unwrap();
            let canonical = state
                .activity
                .latest(&creator.subject, record)
                .await
                .unwrap();
            let response = crate::http::activity::latest(
                State(state.clone()),
                creator.clone(),
                Path(request.channel.id.to_string()),
            )
            .await;
            let body = axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap();
            assert_public(&body, &canonical, label);
        }
    }
}

#[tokio::test]
async fn relay_and_other_provider_receipts_use_the_same_public_projection() {
    use cmaeg_core::channel::Platform;
    let (_server, state, mut request, creator) = fixture_app().await;
    private_actor(&mut request);
    let record = request.channel.definition.record_id.as_str();
    state.activity.received(&request).await.unwrap();
    for source in [
        SourceKind::NyxidRelay {
            platform: Platform::Lark,
            route_id: "fixture-route".into(),
        },
        SourceKind::NyxidRelay {
            platform: Platform::Feishu,
            route_id: "fixture-route".into(),
        },
        SourceKind::Discord {
            bot_id: "123".into(),
        },
    ] {
        state
            .activity
            .modify(&creator.subject, record, today(), |bucket| {
                bucket.items[0].source = source.clone();
                bucket.items[0].actor.display_name = None;
            })
            .await
            .unwrap();
        let canonical = state
            .activity
            .latest(&creator.subject, record)
            .await
            .unwrap();
        let response = crate::http::activity::latest(
            State(state.clone()),
            creator.clone(),
            Path(request.channel.id.to_string()),
        )
        .await;
        assert_public(
            &axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap(),
            &canonical,
            "Participant",
        );
        let response = crate::http::activity::stream(
            State(state.clone()),
            creator.clone(),
            Path(request.channel.id.to_string()),
            Query(Default::default()),
            HeaderMap::new(),
        )
        .await;
        let mut stream = response.into_body().into_data_stream();
        assert_frame(
            &stream.next().await.unwrap().unwrap(),
            &canonical,
            "activity",
        );
    }
}

#[tokio::test]
async fn public_reads_preserve_execution_identity_and_masked_console_diagnostics() {
    use crate::{
        console::observer::{ConsoleObserver, Observation},
        turns::observer::ActivityObserver,
    };
    let (_server, state, mut request, creator) = fixture_app().await;
    private_actor(&mut request);
    let canonical = request.clone();
    let (console, mut queue) = ConsoleObserver::new();
    let observer = Arc::new(ObservedActivity::new(
        state.activity.clone(),
        Arc::new(console),
    ));
    observer.receive(&request).await.unwrap();
    let before = state
        .activity
        .latest(
            &creator.subject,
            request.channel.definition.record_id.as_str(),
        )
        .await
        .unwrap();
    let response = crate::http::activity::latest(
        State(state.clone()),
        creator,
        Path(request.channel.id.to_string()),
    )
    .await;
    assert_public(
        &axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap(),
        &before,
        "Participant",
    );
    struct Launcher {
        expected: LaunchRequest,
        calls: Arc<AtomicUsize>,
    }
    impl TurnLauncher for Launcher {
        fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
            Box::pin(async move {
                assert_eq!(request.activity, self.expected.activity);
                assert_eq!(request.event_ref, self.expected.event_ref);
                assert_eq!(request.conversation_id, self.expected.conversation_id);
                assert_eq!(request.idempotency_key, self.expected.idempotency_key);
                assert_eq!(
                    request
                        .activity
                        .partition_key(&request.channel.definition.partition),
                    self.expected
                        .activity
                        .partition_key(&self.expected.channel.definition.partition)
                );
                let opened = request
                    .event_ref
                    .open(&fixture::keys(), fixture::NOW)
                    .unwrap();
                assert_eq!(
                    opened.payload.activity_sha256,
                    Some(cmaeg_core::seal::event_ref::activity_digest(
                        &request.activity
                    ))
                );
                self.calls.fetch_add(1, Ordering::SeqCst);
                LaunchOutcome::Accepted {
                    response_id: "response-fixture".into(),
                }
            })
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let launcher = ObservedLauncher::new(
        Arc::new(Launcher {
            expected: canonical,
            calls: calls.clone(),
        }),
        observer,
    );
    assert!(matches!(
        launcher.launch(request.clone()).await,
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let observation = queue.try_next().unwrap();
    let Observation::Routed { entry_seed, .. } = &observation else {
        panic!("missing console admission")
    };
    assert_eq!(entry_seed.actor_masked, "…inel");
    assert_eq!(entry_seed.chat_masked, "…");
    assert_ne!(entry_seed.event_key, request.activity.event_id.as_str());
    assert_eq!(entry_seed.source, "native_lark");
    assert_eq!(entry_seed.kind, "message");
    let dump = serde_json::to_string(&observation).unwrap();
    for raw in [ACTOR_ID, ACTOR_NAME, request.activity.event_id.as_str()] {
        assert!(!dump.contains(raw));
    }
    assert!(matches!(
        queue.try_next(),
        Some(Observation::Launch {
            outcome: crate::turns::observer::LaunchClass::Accepted,
            ..
        })
    ));
    let after = state
        .activity
        .latest(
            &request.channel.definition.owner.subject,
            request.channel.definition.record_id.as_str(),
        )
        .await
        .unwrap();
    assert_eq!(after.items[0].actor, request.activity.actor);
    assert_eq!(after.items[0].received_at, before.items[0].received_at);
}
