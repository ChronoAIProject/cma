use super::{custom_preparation_fixture::App, *};
use cmaeg_core::activity::{ActivityKind, ActorKind, SourceKind};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn custom_http_fresh_authenticated_event_reserves_once_and_replays_exact_intent() {
    let mut app = App::new(true).await;
    let url = app.listen(true, true).await;
    let event = App::event("fresh-custom-positive");
    assert_eq!(
        App::request(&url, event.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let plan = app.plan("fresh-custom-positive").await;
    assert_eq!(plan.activity.actor.kind, ActorKind::System);
    assert_eq!(plan.activity.source, SourceKind::Custom);
    assert!(matches!(plan.activity.kind, ActivityKind::Event { .. }));
    assert!(plan.permits_binding_preparation());
    assert_eq!(plan.targets[0].binding_replacement_version, Some(1));
    assert!(plan.binding_custom_source.is_some());
    assert!(matches!(
        app.dispatch(&plan).await,
        Err(ExecutionError::Unavailable)
    ));
    assert!(app.port.base.starts.lock().unwrap().is_empty());
    let DispatchOutcome::Replay {
        mut lease, body, ..
    } = app.dispatch(&plan).await.unwrap()
    else {
        panic!("expected accepted replacement generation");
    };
    assert_eq!(body.binding_generation, Some(4));
    app.dispatcher.journal().release(&mut lease).await.unwrap();
    assert_eq!(
        App::request(&url, event.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    assert_eq!(app.plan("fresh-custom-positive").await, plan);
    let DispatchOutcome::Replay { mut lease, .. } = app.dispatch(&plan).await.unwrap() else {
        panic!("expected replay");
    };
    app.dispatcher.journal().release(&mut lease).await.unwrap();
    assert_eq!(app.port.inspect_count.load(Ordering::SeqCst), 1);
    let intents = app.port.reservations.lock().unwrap();
    assert_eq!(intents.len(), 2);
    assert_eq!(intents[0], intents[1]);
    assert_eq!(app.port.base.starts.lock().unwrap().len(), 1);
    let debug = format!("{plan:?}");
    assert!(!debug.contains("fingerprint"));
    assert!(!debug.contains("Zml4dHVyZQ=="));
}

#[tokio::test]
async fn custom_http_old_admission_never_gains_replacement_when_later_enrolled() {
    let mut app = App::new(false).await;
    let disabled = app.listen(false, false).await;
    let event = App::event("old-custom-admission");
    assert_eq!(
        App::request(&disabled, event.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let original = app.plan("old-custom-admission").await;
    assert!(original.binding_preparation_version.is_none());
    assert!(original.binding_custom_source.is_none());
    let enabled = app.listen(true, true).await;
    assert_eq!(
        App::request(&enabled, event.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    assert_eq!(app.plan("old-custom-admission").await, original);
    let DispatchOutcome::Replay { mut lease, .. } = app.dispatch(&original).await.unwrap() else {
        panic!("legacy dispatch");
    };
    app.dispatcher.journal().release(&mut lease).await.unwrap();
    assert!(app.port.reservations.lock().unwrap().is_empty());
    assert_eq!(app.port.inspect_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn custom_http_producer_rotation_or_consumer_revocation_prevents_reservation() {
    for producer in [true, false] {
        let mut app = App::new(false).await;
        let url = app.listen(true, true).await;
        let event = App::event("revoke-before-reservation");
        assert_eq!(
            App::request(&url, event.to_string())
                .send()
                .await
                .unwrap()
                .status(),
            202
        );
        let plan = app.plan("revoke-before-reservation").await;
        app.revoke_on_inspection(producer);
        assert!(
            matches!(
                app.dispatch(&plan).await,
                Err(ExecutionError::AuthorityChanged)
            ),
            "producer={producer}"
        );
        assert_eq!(app.port.inspect_count.load(Ordering::SeqCst), 1);
        assert!(app.port.reservations.lock().unwrap().is_empty());
        assert!(app.port.base.starts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn custom_http_forged_payload_or_internal_event_cannot_authorize_replacement() {
    let mut app = App::new(false).await;
    let url = app.listen(true, false).await;
    let mut event = App::event("payload-cannot-enroll");
    event["data"] =
        json!({"actor":{"kind":"human"},"binding_replacement_version":1,"replacement":true});
    assert_eq!(
        App::request(&url, event.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let plan = app.plan("payload-cannot-enroll").await;
    assert!(plan.targets[0].binding_replacement_version.is_none());
    assert!(plan.binding_custom_source.is_none());
    assert!(matches!(
        app.dispatch(&plan).await,
        Err(ExecutionError::Invalid) | Err(ExecutionError::Unsupported)
    ));
    assert!(app.port.reservations.lock().unwrap().is_empty());
    let enrolled = app.listen(true, true).await;
    for kind in [
        "cma.message.updated.v1",
        "cma.message.deleted.v1",
        "cma.message.reaction.updated.v1",
        "cma.callback.v1",
    ] {
        let mut event = App::event(kind);
        event["type"] = kind.into();
        event["data"] = json!({"original_event_id":"old-event"});
        assert_eq!(
            App::request(&enrolled, event.to_string())
                .send()
                .await
                .unwrap()
                .status(),
            202
        );
        let plan = app.plan(kind).await;
        assert!(!plan.permits_binding_preparation());
        assert!(plan.targets[0].binding_replacement_version.is_none());
        assert!(plan.binding_custom_source.is_none());
    }
    let response = App::request(&enrolled, App::event("bad-auth").to_string())
        .header("webhook-signature", "v1,invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}
