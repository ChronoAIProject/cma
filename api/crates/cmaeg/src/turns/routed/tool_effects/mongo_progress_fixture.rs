//! Shared test lifecycle and transport fixtures for the replica progress regressions.
use super::{super::replica_test_fixture::*, *};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
        routing_store::Candidate,
    },
    turns::routed::{
        delivery_tests::command,
        scheduler::{Disposition, TargetExecutor, WorkItem},
        test_support, FrozenTurn, FrozenTurnBuilder,
    },
};
use cmaeg_core::{ids::EventId, seal::EventRef};
use futures_util::{future::BoxFuture, FutureExt};
use std::{
    collections::BTreeSet,
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(super) async fn with_harness(test: impl for<'a> FnOnce(&'a mut Harness) -> BoxFuture<'a, ()>) {
    run_harness(Harness::new().await, test).await;
}
pub(super) async fn with_request_harness(
    request: crate::ingress::launch::LaunchRequest,
    test: impl for<'a> FnOnce(&'a mut Harness) -> BoxFuture<'a, ()>,
) {
    run_harness(Harness::with_request(request, false).await, test).await;
}
pub(super) async fn with_partitioned_harness(
    test: impl for<'a> FnOnce(&'a mut Harness) -> BoxFuture<'a, ()>,
) {
    let request = fixture::request(
        |v| {
            v["routing"]["default_targets"] = serde_json::json!(["default"]);
            v["secrets"]["telegram_bot_token"] = serde_json::json!("77:fixture-only");
            v["partition"] = serde_json::json!({"version":1,"key":"conversation_and_sender","custom_key":"fixed"});
        },
        fixture::message("partition fixture"),
    );
    run_harness(Harness::with_request(request, false).await, test).await;
}
pub(super) async fn run_harness(
    mut h: Harness,
    test: impl for<'a> FnOnce(&'a mut Harness) -> BoxFuture<'a, ()>,
) {
    let outcome = AssertUnwindSafe(test(&mut h)).catch_unwind().await;
    h.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

pub(super) struct Task<T>(Option<tokio::task::JoinHandle<T>>);
impl<T> Task<T> {
    pub fn new(task: tokio::task::JoinHandle<T>) -> Self {
        Self(Some(task))
    }
    pub fn abort(&self) {
        self.0.as_ref().unwrap().abort();
    }
    pub async fn join(mut self) -> T {
        self.0.take().unwrap().await.unwrap()
    }
}
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

struct NoParent;
impl TargetExecutor for NoParent {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { panic!("delayed reply must not dispatch its parent") })
    }
}

pub(super) async fn sweep(h: &Harness, id: &str) {
    let work = WorkItem {
        identity: h.plan.identity.clone(),
        target_id: id.into(),
    };
    for (node, tools) in h.nodes.iter().zip(&h.tools) {
        let executor = node
            .journal()
            .effect_executor(
                node.plans(),
                Arc::new(NoParent),
                tools.replica_effect_executor(),
            )
            .unwrap();
        if matches!(executor.execute(&work).await, Disposition::Settled) {
            return;
        }
    }
}

pub(super) fn transport(h: &mut Harness, delay: Duration) -> (Task<()>, Arc<AtomicUsize>) {
    let mut rx = std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1);
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let task = tokio::spawn(async move {
        loop {
            let reply = command(&mut rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit(), "expired worker must not initiate IO");
            count.fetch_add(1, Ordering::SeqCst);
            // The request has crossed the transport boundary. A slow receipt
            // does not imply the platform refused the already transmitted send.
            tokio::time::sleep(delay).await;
            let _ = reply.receipt.send(receipt());
        }
    });
    (Task::new(task), attempts)
}

pub(super) fn effect_id(
    h: &Harness,
    capability: &cmaeg_core::seal::ReplyTargetPayload,
    key: &str,
) -> String {
    model::work_id(
        &model::operation(
            h.target(),
            &Payload {
                capability: capability.clone(),
                caller: caller().key_hash,
                message: message(),
            },
            key,
        )
        .unwrap(),
    )
}

pub(super) async fn later_plan(h: &Harness) -> AdmittedPlan {
    plan_for_conversation(h, "later-event", "42").await
}
pub(super) async fn plan_for_conversation(
    h: &Harness,
    event_id: &str,
    conversation: &str,
) -> AdmittedPlan {
    plan_with_expiry(h, event_id, conversation, h.event().expires_at).await
}
pub(super) async fn plan_with_expiry(
    h: &Harness,
    event_id: &str,
    conversation: &str,
    expires_at: i64,
) -> AdmittedPlan {
    let mut activity = fixture::message("later independent turn");
    activity.event_id = EventId::new(event_id).unwrap();
    activity.conversation.id = conversation.into();
    activity.conversation.kind = h.plan.activity.conversation.kind;
    activity.source = h.plan.activity.source.clone();
    let mut request = fixture::request(
        |v| {
            v["routing"]["default_targets"] = serde_json::json!(["default"]);
        },
        activity,
    );
    request.channel = h.current.channel.lock().unwrap().clone();
    let mut event = request
        .event_ref
        .open(&fixture::keys(), fixture::NOW)
        .unwrap()
        .payload;
    event.channel_id = request.channel.id.clone();
    event.record_id = request.channel.pointer.record_id.clone();
    event.expires_at = expires_at;
    event.partition_key = request
        .activity
        .partition_key(&request.channel.definition.partition);
    event.conversation_id = cmaeg_core::ids::ConversationId::derive(
        &h.database.keys(),
        &request.channel.id,
        1,
        &event.partition_key,
    );
    request.conversation_id = event.conversation_id.clone();
    request.event_ref = EventRef::seal(&event, &h.database.keys()).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let OrdinaryPlan::Routed(plan) = routing_plan::plan(
        &request,
        &h.database.keys(),
        now,
        RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &BTreeSet::from(["cma_codex".into()]),
        },
    )
    .unwrap() else {
        panic!("routed later event")
    };
    let candidate = Candidate::prepare(
        &request,
        &plan,
        b"later-independent-event",
        &h.database.keys(),
        now,
    )
    .unwrap();
    h.nodes[0].plans().admit(&candidate).await.unwrap()
}

pub(super) struct Builder(pub Arc<cmaeg_core::seal::MasterKeyRing>);
impl FrozenTurnBuilder for Builder {
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            let mut body = test_support::frozen(plan, target, current, 32);
            body.event_context.event_ref = FrozenTurn::event_ref(plan, target, &self.0)?;
            Ok(body)
        })
    }
}
