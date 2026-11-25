//! Single-target compatibility keeps authenticated legacy identity and queue custody.
use super::*;
use crate::ingress::{
    routing_plan::fixture,
    routing_store::{AdmissionError, AdmissionQueue, AdmittedPlan},
};
use futures_util::future::BoxFuture;
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Default)]
struct Queue(AtomicBool);
impl AdmissionQueue for Queue {
    fn enqueue<'a>(&'a self, _: &'a AdmittedPlan) -> BoxFuture<'a, Result<(), AdmissionError>> {
        Box::pin(async {
            if self.0.load(Ordering::SeqCst) {
                Err(AdmissionError::Uncertain)
            } else {
                Ok(())
            }
        })
    }
}
fn enabled(setup: &mut Setup) -> Arc<Queue> {
    let queue = Arc::new(Queue::default());
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .plans = setup.server.store().with_admission_queue(queue.clone());
    queue
}

#[tokio::test]
async fn command_refusal_precedes_every_new_default_dispatch() {
    let request = fixture::request(|_| {}, fixture::message("/to absent-fixture hello"));
    for (selected, replicated) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut setup = Setup::new(&request, selected).await;
        if replicated {
            enabled(&mut setup);
        }
        assert!(matches!(
            setup.launch(request.clone(), b"wire", true).await,
            LaunchOutcome::Refused(Problem { status: 403, detail: Some(detail), .. })
                if detail == "target_unknown"
        ));
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
        assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
    }
}
async fn one(setup: &Setup) -> AdmittedPlan {
    let page = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert!(page.issues.is_empty());
    assert_eq!(page.plans.len(), 1);
    page.plans.into_iter().next().unwrap()
}

async fn assert_command_replay(setup: &Setup, request: LaunchRequest, original: AdmittedPlan) {
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let expected = (
        original.admission_id.clone(),
        request.conversation_id.clone(),
    );
    assert_eq!(original.targets.len(), 1);
    assert!(original.targets[0].launch.consumer.is_default());
    assert_eq!(
        original.targets[0].launch.idempotency_key,
        request.idempotency_key
    );
    assert_eq!(original.source_body, b"original command bytes");
    assert_eq!(
        admitted(
            setup
                .launch(request.clone(), &original.source_body, true)
                .await
        ),
        expected
    );
    assert!(matches!(
        setup
            .launch(request.clone(), b"changed command bytes", true)
            .await,
        LaunchOutcome::Refused(Problem { status: 409, .. })
    ));

    let mut fresh = request;
    fresh.activity.event_id = fixture::event_id("another-event");
    fresh.idempotency_key = cmaeg_core::ids::IdempotencyKey::from_event(&fresh.activity.event_id);
    let activity = fresh.activity.clone();
    fixture::rewrite_ref(&mut fresh, |event| {
        event.event_id = activity.event_id.clone();
        event.activity_sha256 = Some(activity_digest(&activity));
    });
    assert!(matches!(
        setup.launch(fresh, &original.source_body, true).await,
        LaunchOutcome::Refused(Problem { status: 403, detail: Some(detail), .. })
            if detail == "target_unknown"
    ));
    assert_eq!(one(setup).await, original);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn retained_compatibility_command_replays_before_current_command_policy() {
    for selected in [false, true] {
        let mut request = fixture::request(
            |value| value["routing"]["addressing"]["command"] = false.into(),
            fixture::message("/to absent-fixture hello"),
        );
        let mut setup = Setup::new(&request, false).await;
        enabled(&mut setup);
        admitted(
            setup
                .launch(request.clone(), b"original command bytes", true)
                .await,
        );
        let original = one(&setup).await;
        assert!(original.is_compatibility());
        Arc::make_mut(&mut request.channel)
            .definition
            .routing
            .as_mut()
            .unwrap()
            .addressing
            .command = true;
        if selected {
            Arc::get_mut(setup.state.routed.as_mut().unwrap())
                .unwrap()
                .records
                .insert(request.channel.pointer.record_id.to_string());
        }
        assert_command_replay(&setup, request, original).await;
    }
}

#[tokio::test]
async fn admitted_default_alias_replays_after_alias_rename() {
    for selected in [false, true] {
        let mut request = fixture::request(|_| {}, fixture::message("/to lead hello"));
        let mut setup = Setup::new(&request, selected).await;
        enabled(&mut setup);
        admitted(
            setup
                .launch(request.clone(), b"original command bytes", true)
                .await,
        );
        let original = one(&setup).await;
        assert_eq!(original.is_compatibility(), !selected);
        Arc::make_mut(&mut request.channel)
            .definition
            .routing
            .as_mut()
            .unwrap()
            .default_consumer
            .name = cmaeg_core::ids::ConsumerName::parse("renamed").unwrap();
        assert_command_replay(&setup, request, original).await;
    }
}
fn interactive() -> LaunchRequest {
    let mut request = request();
    let decision = fixture::routed(&request);
    let target = decision
        .targets
        .iter()
        .find(|t| !t.consumer.is_default())
        .unwrap();
    request.activity.kind = serde_json::from_value(
        json!({"type":"interaction","block_id":"card","action_id":"approve","values":{"choice":"yes"}}),
    )
    .unwrap();
    request.activity.routing = decision.activity.routing;
    request.consumer = Some(target.consumer.clone());
    request.conversation_id = target.event.conversation_id.clone();
    request.idempotency_key = target.idempotency_key.clone();
    let mut event = target.event.clone();
    event.activity_sha256 = Some(activity_digest(&request.activity));
    request.event_ref = cmaeg_core::seal::EventRef::seal(&event, &fixture::keys()).unwrap();
    request
}

#[tokio::test]
async fn enabled_unselected_and_unrouted_requests_preserve_original_single_target() {
    for selected in [false, true] {
        let mut request = request();
        if selected {
            Arc::get_mut(&mut request.channel)
                .unwrap()
                .definition
                .routing = None;
        }
        let original = request.clone();
        let mut setup = Setup::new(&request, selected).await;
        enabled(&mut setup);
        let outcome = admitted(setup.launch(request, b"original-wire", true).await);
        let plan = one(&setup).await;
        assert!(plan.is_compatibility());
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.activity, original.activity);
        assert_eq!(plan.source_body, b"original-wire");
        assert_eq!(
            plan.targets[0].launch.event,
            original
                .event_ref
                .open(&fixture::keys(), fixture::NOW)
                .unwrap()
                .payload
        );
        assert_eq!(
            plan.targets[0].launch.idempotency_key,
            original.idempotency_key
        );
        assert_eq!(outcome.1, original.conversation_id);
        assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            admitted(setup.launch(original, b"original-wire", true).await),
            outcome
        );
    }
}

#[tokio::test]
async fn enabled_callback_keeps_authenticated_owner_without_fanout_or_rerouting() {
    let request = interactive();
    let mut setup = Setup::new(&request, true).await;
    enabled(&mut setup);
    let outcome = admitted(
        setup
            .launch(request.clone(), b"signed-callback", true)
            .await,
    );
    let plan = one(&setup).await;
    assert_eq!(plan.activity, request.activity);
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(
        plan.targets[0].launch.consumer,
        request.consumer.clone().unwrap()
    );
    assert_eq!(
        plan.targets[0].launch.idempotency_key,
        request.idempotency_key
    );
    assert_eq!(
        plan.targets[0].launch.event,
        request
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload
    );
    let mut changed = request.clone();
    Arc::make_mut(&mut changed.channel).definition.routing = None;
    assert_eq!(
        admitted(setup.launch(changed, b"signed-callback", true).await),
        outcome
    );
    assert!(matches!(
        setup.launch(request, b"changed-wire", true).await,
        LaunchOutcome::Refused(Problem { status: 409, .. })
    ));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn compatibility_queue_failure_never_accepts_or_falls_back_and_restart_repairs_root() {
    let request = request();
    let mut setup = Setup::new(&request, false).await;
    let queue = enabled(&mut setup);
    queue.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        setup.launch(request.clone(), b"wire", true).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    let plan = one(&setup).await; // Inspection without queue injection sees retained root.
    let permit = setup.state.inflight.clone().try_acquire_owned().unwrap();
    assert!(matches!(
        setup
            .state
            .launch_verified(request.clone(), None, permit)
            .await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    queue.0.store(false, Ordering::SeqCst);
    let restored = setup
        .server
        .store()
        .with_admission_queue(queue)
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert_eq!(restored.plans, vec![plan.clone()]);
    assert_eq!(
        admitted(setup.launch(request, b"wire", true).await).0,
        plan.admission_id
    );
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn compatibility_rejects_changed_consumer_conversation_partition_or_key_on_replay() {
    let request = interactive();
    let mut setup = Setup::new(&request, false).await;
    enabled(&mut setup);
    admitted(setup.launch(request.clone(), b"wire", true).await);
    for change in 0..4 {
        let mut next = request.clone();
        let mut event = next
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        match change {
            0 => {
                next.consumer = None;
                event.consumer = None;
            }
            1 => {
                next.conversation_id = cmaeg_core::ids::ConversationId::derive(
                    &fixture::keys(),
                    &next.channel.id,
                    1,
                    "other",
                );
                event.conversation_id = next.conversation_id.clone();
            }
            2 => event.partition_key = "another-partition".into(),
            _ => {
                next.idempotency_key =
                    cmaeg_core::ids::IdempotencyKey::from_event(&fixture::event_id("another-key"))
            }
        }
        next.event_ref = cmaeg_core::seal::EventRef::seal(&event, &fixture::keys()).unwrap();
        assert!(
            matches!(
                setup.launch(next, b"wire", true).await,
                LaunchOutcome::Refused(Problem { status: 409, .. })
            ),
            "change {change}"
        );
    }
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn completion_adapter_admits_original_system_event_and_requires_queue() {
    use crate::turns::observer::ActivityObserver;
    let mut request = fixture::request(
        |v| {
            v["routing"] = serde_json::Value::Null;
            v["secrets"]["consumer_keys"] = json!([]);
        },
        fixture::custom(None),
    );
    let ActivityKind::Event { ce, .. } = &mut request.activity.kind else {
        unreachable!()
    };
    ce.r#type = "gateway.connection.ready".into();
    let digest = activity_digest(&request.activity);
    fixture::rewrite_ref(&mut request, |e| e.activity_sha256 = Some(digest));
    let mut setup = Setup::new(&request, false).await;
    let observer: Arc<dyn ActivityObserver> = setup.state.activity.clone();
    let launcher = setup
        .state
        .routed
        .as_ref()
        .unwrap()
        .replica_completion_launcher(observer.clone());
    assert!(matches!(
        launcher.launch(request.clone()).await,
        LaunchOutcome::Refused(_)
    ));
    drop(launcher);
    let queue = enabled(&mut setup);
    let launcher = setup
        .state
        .routed
        .as_ref()
        .unwrap()
        .replica_completion_launcher(observer);
    queue.0.store(true, Ordering::SeqCst);
    assert!(matches!(
        launcher.launch(request.clone()).await,
        LaunchOutcome::Refused(Problem { status: 503, .. })
    ));
    queue.0.store(false, Ordering::SeqCst);
    let outcome = admitted(launcher.launch(request.clone()).await);
    assert_eq!(outcome.1, request.conversation_id);
    let plan = one(&setup).await;
    assert_eq!(plan.activity, request.activity);
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(admitted(launcher.launch(request).await), outcome);
}
