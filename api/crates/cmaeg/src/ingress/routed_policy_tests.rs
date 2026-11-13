//! Admission gates and unchanged OFF-path policy refusals.
use super::*;

#[tokio::test]
async fn unproved_provider_replay_is_refused_before_durable_admission() {
    let mut setup = Setup::new(&request(), true).await;
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .replay_providers
        .clear();
    assert!(matches!(
        setup.launch(request(), b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsupported_delivery_never_commits_or_launches_but_retries_keep_original_plan() {
    let mut sync = request();
    fixture::rewrite_ref(&mut sync, |event| {
        event.reply = cmaeg_core::seal::ReplyAddress::Sync {
            waiter: "ephemeral-waiter".into(),
        };
    });
    let setup = Setup::new(&sync, true).await;
    assert!(matches!(
        setup.launch(sync, b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 422, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);

    let first = admitted(setup.launch(request(), b"wire", true).await);
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let mut changed = request();
    Arc::get_mut(&mut changed.channel)
        .unwrap()
        .definition
        .routing
        .as_mut()
        .unwrap()
        .consumers[0]
        .reply
        .destination = cmaeg_core::channel::ReplyChoice::Default;
    assert_eq!(admitted(setup.launch(changed, b"wire", true).await), first);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn custom_sync_admission_requires_the_explicit_shared_stream_composition() {
    let request = || {
        let mut request = fixture::request(
            |value| {
                fixture::targets(value);
                value["reply"]["sinks"] = serde_json::json!([{"type":"sync"}]);
            },
            fixture::custom(None),
        );
        fixture::rewrite_ref(&mut request, |event| {
            event.reply = cmaeg_core::seal::ReplyAddress::Sync {
                waiter: "shared-custom-waiter".into(),
            };
        });
        request
    };
    let mut setup = Setup::new(&request(), true).await;
    assert!(matches!(
        setup.launch(request(), b"custom-event", true).await,
        LaunchOutcome::Refused(Problem { status: 422, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .sync_delivery_enabled = true;
    admitted(setup.launch(request(), b"custom-event", true).await);
    assert_eq!(
        setup
            .server
            .store()
            .recover_all_page(None, 32)
            .await
            .unwrap()
            .plans
            .len(),
        1
    );
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pausing_admission_keeps_committed_retries_and_refuses_new_work_before_writes() {
    let mut setup = Setup::new(&request(), true).await;
    let first = admitted(setup.launch(request(), b"wire", true).await);
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .admission_enabled = false;
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    assert_eq!(
        admitted(setup.launch(request(), b"wire", true).await),
        first
    );
    let mut another = request();
    another.activity.event_id = fixture::event_id("new-event");
    another.idempotency_key =
        cmaeg_core::ids::IdempotencyKey::from_event(&another.activity.event_id);
    let activity = another.activity.clone();
    fixture::rewrite_ref(&mut another, |event| {
        event.event_id = activity.event_id.clone();
        event.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(&activity));
    });
    assert!(matches!(
        setup.launch(another, b"new-wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stopped_scheduler_refuses_new_admission_before_writing() {
    let request = request();
    let setup = Setup::new(&request, true).await;
    drop(setup._scheduler);
    let captured = setup.state.capture_routed(&request, b"wire", true);
    let permit = setup.state.inflight.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        setup.state.launch_verified(request, captured, permit).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_selected_body_is_not_copied_or_admitted() {
    let request = request();
    let setup = Setup::new(&request, true).await;
    let bytes = vec![b'x'; MAX_SOURCE_BYTES + 1];
    let captured = setup.state.capture_routed(&request, &bytes, true).unwrap();
    assert!(captured.body.is_empty());
    assert!(captured.oversized);
    let permit = setup.state.inflight.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        setup
            .state
            .launch_verified(request, Some(captured), permit)
            .await,
        LaunchOutcome::Refused(Problem { status: 413, .. })
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unsigned_binary_custom_target_attributes_cannot_select_consumers() {
    let request = fixture::request(fixture::targets, fixture::custom(Some(fixture::REVIEWER)));
    let setup = Setup::new(&request, true).await;
    assert!(matches!(
        setup.launch(request, b"binary data", false).await,
        LaunchOutcome::Refused(_)
    ));
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}
