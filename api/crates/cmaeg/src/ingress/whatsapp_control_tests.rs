use super::{
    launch::{LaunchOutcome, UnavailableLauncher},
    routed::RoutedIngress,
    routing_plan::fixture,
    routing_store::EventIdentity,
    IngressState,
};
use crate::{
    channels::resolver::ChannelResolver,
    interactions::whatsapp_native_fixture::Fixture,
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
    whatsapp::{
        tests::{envelope, text},
        webhook,
    },
};
use cmaeg_core::activity::ActivityKind;
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
struct Executor;
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { Disposition::Settled })
    }
}
fn ingress(f: &Fixture) -> (IngressState, Scheduler) {
    let keys = Arc::new(fixture::keys());
    let (scheduler, handle) = Scheduler::new(
        Arc::new(f.server.store()),
        Arc::new(Executor),
        ScheduleConfig::default(),
    )
    .unwrap();
    let routed = RoutedIngress::new(
        f.server.store(),
        handle,
        keys.clone(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::from(["cma_codex".into(), "nyxbot".into()]),
    );
    let state = IngressState::new(
        ChannelResolver::new(
            Arc::new(f.server.records.clone()),
            keys.clone(),
            Default::default(),
        ),
        keys,
        Default::default(),
        Arc::new(UnavailableLauncher),
    )
    .unwrap()
    .with_routed(Some(Arc::new(routed)))
    .with_interactions(f.store.clone());
    (state, scheduler)
}
fn identity(f: &Fixture, id: &str) -> EventIdentity {
    EventIdentity {
        owner: f.channel.definition.owner.subject.clone(),
        channel_id: f.channel.id.clone(),
        record_id: f.channel.definition.record_id.clone(),
        event_id: webhook::event_id(&crate::whatsapp::tests::source(), id).unwrap(),
    }
}
#[tokio::test]
async fn native_whatsapp_controls_commit_only_original_owner_and_restore_rebatched_callback() {
    for named in [false, true] {
        for list in [false, true] {
            let mut f = Fixture::new(named).await;
            let card = f.card(list, true).await;
            let locator = Fixture::locator(&card, list);
            let raw = f.raw(&locator, "wamid.click");
            let body = envelope(vec![raw.clone()]);
            let original = f.event.clone();
            let channel = Arc::make_mut(&mut f.channel);
            channel.definition.partition.version += 1;
            channel.definition.routing.as_mut().unwrap().default_targets =
                vec![cmaeg_core::ids::ConsumerId::parse(fixture::NYX).unwrap()];
            let (state, _scheduler) = ingress(&f);
            assert!(matches!(
                state
                    .launch_whatsapp_verified(
                        f.verified(raw.clone(), vec![]),
                        state.inflight.clone().try_acquire_owned().unwrap()
                    )
                    .await,
                LaunchOutcome::Admitted { .. }
            ));
            let first = f
                .server
                .store()
                .load(&identity(&f, "wamid.click"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(first.source_body, body);
            assert!(matches!(first.activity.kind, ActivityKind::Interaction(_)));
            assert_eq!(first.targets.len(), 1);
            assert_eq!(first.targets[0].launch.event.consumer, original.consumer);
            assert_eq!(
                first.targets[0].launch.event.conversation_id,
                original.conversation_id
            );
            assert_eq!(
                first.targets[0].launch.event.partition_key,
                original.partition_key
            );
            let writes = f.server.memory.writes.load(Ordering::SeqCst);
            assert!(matches!(
                state
                    .launch_whatsapp_verified(
                        f.verified(raw, vec![text("wamid.extra", "other batch")]),
                        state.inflight.clone().try_acquire_owned().unwrap()
                    )
                    .await,
                LaunchOutcome::Admitted { .. }
            ));
            let replay = f
                .server
                .store()
                .load(&identity(&f, "wamid.click"))
                .await
                .unwrap()
                .unwrap();
            assert!(replay == first);
            assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
        }
    }
}
#[tokio::test]
async fn native_whatsapp_signed_controls_use_current_secret_and_retry_pending_binding() {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    let mut f = Fixture::new(false).await;
    let mut card = f.card(false, false).await;
    let locator = Fixture::locator(&card, false);
    let body = envelope(vec![f.raw(&locator, "wamid.http-click")]);
    let current_secret = "rotated-fixture-app-secret";
    Arc::make_mut(&mut f.channel)
        .definition
        .secrets
        .whatsapp
        .as_mut()
        .unwrap()
        .app_secret = cmaeg_core::channel::Secret::new(current_secret);
    f.save_channel();
    let (state, _scheduler) = ingress(&f);
    let app = crate::http::whatsapp::router(state);
    let request = |valid: bool| {
        let key = if valid {
            current_secret.to_owned()
        } else {
            crate::whatsapp::tests::credentials()
                .app_secret
                .as_str()
                .to_owned()
        };
        let tag = ring::hmac::sign(
            &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes()),
            &body,
        );
        Request::builder()
            .method("POST")
            .uri(format!("/v1/whatsapp/{}", f.channel.id))
            .header("content-type", "application/json")
            .header(
                "x-hub-signature-256",
                format!("sha256={}", webhook::hex(tag.as_ref())),
            )
            .body(Body::from(body.clone()))
            .unwrap()
    };
    let writes = f.server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        call(app.clone(), request(false)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(
        call(app.clone(), request(true)).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    f.store
        .bind(
            &mut card,
            crate::interactions::Destination {
                outbound_id: "wamid.confirmed".into(),
                platform_id: "wamid.confirmed".into(),
            },
        )
        .await
        .unwrap();
    let response = call(app.clone(), request(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &to_bytes(response.into_body(), 8192).await.unwrap()
        )
        .unwrap(),
        serde_json::json!({"accepted":1,"refused":0})
    );
    assert_eq!(
        f.server
            .store()
            .load(&identity(&f, "wamid.http-click"))
            .await
            .unwrap()
            .unwrap()
            .source_body,
        body
    );
    assert_eq!(call(app, request(true)).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn native_whatsapp_controls_recover_lost_admission_ack_after_consumption_once() {
    for lose_read in [false, true] {
        let f = Fixture::new(false).await;
        let card = f.card(false, true).await;
        let locator = Fixture::locator(&card, false);
        let raw = f.raw(&locator, "wamid.lost-admission");
        let identity = identity(&f, "wamid.lost-admission");
        f.server.fault(identity.admission_id(), true, lose_read);
        let (state, _scheduler) = ingress(&f);
        let first = state
            .launch_whatsapp_verified(
                f.verified(raw.clone(), vec![]),
                state.inflight.clone().try_acquire_owned().unwrap(),
            )
            .await;
        assert!(
            matches!(first, LaunchOutcome::Admitted { .. })
                || matches!(first, LaunchOutcome::Refused(ref problem) if problem.status == 503)
        );
        let consumed = f.store.load(&card.card.id).await.unwrap().0;
        assert!(consumed.consumed.is_some());
        let writes = f.server.memory.writes.load(Ordering::SeqCst);
        let plan = f.server.store().load(&identity).await.unwrap().unwrap();
        for _ in 0..2 {
            assert!(matches!(
                state
                    .launch_whatsapp_verified(
                        f.verified(raw.clone(), vec![text("wamid.extra", "rebatched")]),
                        state.inflight.clone().try_acquire_owned().unwrap()
                    )
                    .await,
                LaunchOutcome::Admitted { .. }
            ));
        }
        assert!(f.server.store().load(&identity).await.unwrap().unwrap() == plan);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(f.server.memory.writes.load(Ordering::SeqCst), writes);
    }
}

async fn call(
    app: axum::Router,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    use axum::body::{to_bytes, Body};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (parts, body) = request.into_parts();
    let response = reqwest::Client::new()
        .request(parts.method, format!("http://{address}{}", parts.uri))
        .headers(parts.headers)
        .body(to_bytes(body, 65536).await.unwrap())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    task.abort();
    axum::response::Response::builder()
        .status(status)
        .body(Body::from(bytes))
        .unwrap()
}
