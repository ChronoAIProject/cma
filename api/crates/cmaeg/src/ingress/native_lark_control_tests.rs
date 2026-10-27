use super::{
    launch::{LaunchOutcome, UnavailableLauncher},
    routed::RoutedIngress,
    routing_plan::fixture as routed,
    IngressState,
};
use crate::{
    channels::resolver::ChannelResolver,
    http::readiness::Readiness,
    interactions::native_lark::fixture::{now, Fixture},
    native_lark::test_support as native,
    tools::message_effects::lark_ledger,
    turns::routed::scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
};
use cmaeg_core::channel::LarkRegion;
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, Instant},
};
struct Executor;
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { Disposition::Settled })
    }
}
fn state(f: &Fixture) -> (IngressState, Scheduler) {
    let keys = Arc::new(routed::keys());
    let (scheduler, handle) = Scheduler::new(
        Arc::new(f.server.store()),
        Arc::new(Executor),
        ScheduleConfig::default(),
    )
    .unwrap();
    let routing = RoutedIngress::new(
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
    .with_routed(Some(Arc::new(routing)))
    .with_interactions(f.store.clone())
    .with_native_lark_intake(true);
    (state, scheduler)
}
async fn http(
    state: &IngressState,
    channel: &str,
    raw: &serde_json::Value,
) -> axum::response::Response {
    let (body, signature) = native::signed(raw, now());
    let request = axum::http::Request::builder()
        .header("content-type", "application/json")
        .header("x-lark-request-timestamp", now().to_string())
        .header("x-lark-request-nonce", "fixture_nonce")
        .header("x-lark-signature", signature)
        .body(axum::body::Body::from(body))
        .unwrap();
    crate::http::native_lark::callback(
        axum::extract::State(state.clone()),
        axum::extract::Path(channel.into()),
        request,
    )
    .await
}
#[tokio::test]
async fn crash_after_consume_recovers_exact_default_and_named_admission_without_lark_retry() {
    for named in [false, true] {
        let f = Fixture::new(named, true, LarkRegion::Feishu).await;
        let card = f.card(true, true).await;
        let click = f.callback(&card, "winning_click", 0);
        f.store
            .consume_native_lark(&f.channel, &click)
            .await
            .unwrap();
        let pending = f
            .store
            .native_lark_pending(&card.card.id, &f.channel)
            .await
            .unwrap();
        let destination = card
            .card
            .native_lark
            .as_ref()
            .unwrap()
            .destination
            .as_ref()
            .unwrap();
        let serial = lark_ledger::Store::new(f.server.records.clone(), Arc::new(routed::keys()));
        let _abandoned_local = serial
            .acquire_control(
                lark_ledger::native_destination(&destination.reply),
                &f.channel.definition.owner.subject,
                destination.retained_until,
                &card.card.id,
            )
            .await
            .unwrap()
            .unwrap();
        let (state, _scheduler) = state(&f);
        // New issuance/intake is off: local recovery still resumes already consumed cards.
        let mut worker = crate::native_lark::control_recovery::Worker::start(
            state.clone().with_native_lark_intake(false),
            Readiness::initialized(),
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if f.store
                    .native_lark_pending(&card.card.id, &f.channel)
                    .await
                    .unwrap()
                    .admitted
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let identity = super::routing_store::EventIdentity {
            owner: f.channel.definition.owner.subject.clone(),
            channel_id: f.channel.id.clone(),
            record_id: f.channel.pointer.record_id.clone(),
            event_id: pending.event.event_id.clone(),
        };
        let plan = f.server.store().load(&identity).await.unwrap().unwrap();
        assert_eq!(plan.activity, pending.activity);
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].launch.event, pending.event);
        assert!(!String::from_utf8(plan.source_body)
            .unwrap()
            .contains("token"));
        let again = state
            .resume_native_lark_control(&card.card.id, f.channel.id.as_str())
            .await;
        assert!(matches!(again, LaunchOutcome::Admitted { .. }));
        assert!(worker.drain(Instant::now() + Duration::from_secs(2)).await);
    }
}
#[tokio::test]
async fn callback_ack_requires_durable_admission_and_external_mutation_blocks_consumption() {
    let f = Fixture::new(false, false, LarkRegion::Feishu).await;
    let card = f.card(false, true).await;
    let click = f.callback(&card, "click_one", 0);
    let (state, _scheduler) = state(&f);
    let serial = lark_ledger::Store::new(f.server.records.clone(), Arc::new(routed::keys()));
    let receipt = card
        .card
        .native_lark
        .as_ref()
        .unwrap()
        .destination
        .as_ref()
        .unwrap();
    let held = serial
        .acquire(
            lark_ledger::native_destination(&receipt.reply),
            &f.channel.definition.owner.subject,
            receipt.retained_until,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(!http(&state, f.channel.id.as_str(), &f.raw(&click))
        .await
        .status()
        .is_success());
    assert!(f
        .store
        .load(&card.card.id)
        .await
        .unwrap()
        .0
        .consumed
        .is_none());
    serial.release(held).await.unwrap();
    let response = http(&state, f.channel.id.as_str(), &f.raw(&click)).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .as_ref(),
        b"{}"
    );
    let mut other = click;
    other.event_id = "distinct_click".into();
    assert!(!http(&state, f.channel.id.as_str(), &f.raw(&other))
        .await
        .status()
        .is_success());
}
#[tokio::test]
async fn failed_card_storage_never_acknowledges_and_lost_consume_ack_remains_discoverable() {
    for store in [false, true] {
        let f = Fixture::new(false, false, LarkRegion::Feishu).await;
        let card = f.card(false, true).await;
        let click = f.callback(&card, "click_one", 0);
        *f.server.memory.fault.lock().unwrap() = Some(super::routing_store::test_server::Fault {
            id: card.card.id.clone(),
            store,
            lose_read: false,
            skip: 0,
        });
        let (state, _scheduler) = state(&f);
        assert!(!http(&state, f.channel.id.as_str(), &f.raw(&click))
            .await
            .status()
            .is_success());
        let current = f.store.load(&card.card.id).await.unwrap().0;
        assert_eq!(current.consumed.is_some(), store);
        if store {
            assert!(matches!(
                state
                    .resume_native_lark_control(&card.card.id, f.channel.id.as_str())
                    .await,
                LaunchOutcome::Admitted { .. }
            ));
        }
    }
}
#[tokio::test]
async fn authenticated_http_challenge_finishes_under_one_second_without_durable_writes() {
    let f = Fixture::new(false, false, LarkRegion::Feishu).await;
    let (state, _scheduler) = state(&f);
    let before = f
        .server
        .memory
        .writes
        .load(std::sync::atomic::Ordering::SeqCst);
    let raw = serde_json::json!({"type":"url_verification","token":native::credentials().verification_token.as_str(),"challenge":"challenge_fixture"});
    let started = Instant::now();
    let response = http(&state, f.channel.id.as_str(), &raw).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        f.server
            .memory
            .writes
            .load(std::sync::atomic::Ordering::SeqCst),
        before
    );
}
