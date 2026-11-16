//! #2171 Phase 1: desired progress guarantees, intentionally red on the baseline.
//! Only the provider HTTP server and final platform transport are fixtures.
use super::{super::replica_test_fixture::*, *};
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::intents::DeliveryOutcome,
    ingress::{
        routing_plan::{self, fixture, OrdinaryPlan, RoutingOptions},
        routing_store::Candidate,
    },
    turns::routed::{
        adapter_test_server::HttpServer,
        delivery_tests::command,
        scheduler::{Disposition, TargetExecutor, WorkItem},
        test_support, DispatchOutcome, ExecutionPort, FrozenTurn, FrozenTurnBuilder,
        HttpExecutionPort,
    },
};
use cmaeg_core::{ids::EventId, seal::EventRef};
use futures_util::future::BoxFuture;
use std::sync::atomic::Ordering;
use std::{collections::BTreeSet, sync::atomic::AtomicUsize};

struct NoParent;
impl TargetExecutor for NoParent {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async { panic!("delayed reply must not dispatch its parent") })
    }
}

async fn sweep(h: &Harness, id: &str) {
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

fn transport(h: &mut Harness, delay: Duration) -> (tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
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
    (task, attempts)
}

fn effect_id(h: &Harness, capability: &cmaeg_core::seal::ReplyTargetPayload, key: &str) -> String {
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

async fn later_plan(h: &Harness) -> AdmittedPlan {
    let mut activity = fixture::message("later independent turn");
    activity.event_id = EventId::new("later-event").unwrap();
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
    event.expires_at = h.event().expires_at;
    event.conversation_id = h.event().conversation_id;
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

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_reply_behind_unknown_head_eventually_returns_receipt_without_parent_settlement() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    let journal = h.nodes[owner].journal();
    journal
        .transition(&mut lease, Phase::Unknown)
        .await
        .unwrap();
    journal.release(&mut lease).await.unwrap();
    let mut capability = h.capability();
    // Membership clocks refresh asynchronously; a capability minted five
    // seconds ago is valid on both clients even across a wall-second boundary.
    capability.issued_at -= 5;
    let key = "behind-unknown";
    let admitted = h.tools[0]
        .reply_delayed(caller(), &capability, key, message())
        .await;
    assert!(
        matches!(admitted, Err(ExecutionError::Busy)),
        "admission: {admitted:?}"
    );
    // Busy is the existing REST 409 routed_reply_in_progress mapping. Retrying
    // the SAME admitted occurrence must eventually produce a terminal receipt.
    let id = effect_id(&h, &capability, key);
    let (sink, attempts) = transport(&mut h, Duration::ZERO);
    let mut result = Err(ExecutionError::Busy);
    let mut pending = 0;
    for _ in 0..3 {
        sweep(&h, &id).await;
        result = h.tools[1]
            .reply_delayed(caller(), &capability, key, message())
            .await;
        if !matches!(result, Err(ExecutionError::Busy)) {
            break;
        }
        pending += 1;
    }
    let (parent, _) = journal
        .read::<TargetState>(&h.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parent.phase,
        Phase::Unknown,
        "reply must not settle the unknown parent"
    );
    assert!(
        attempts.load(Ordering::SeqCst) <= 1,
        "same key must never send twice"
    );
    let sent = attempts.load(Ordering::SeqCst);
    sink.abort();
    h.cleanup().await;
    assert!(
        matches!(result, Ok(Some(_))),
        "#2171: {pending} same-key retries still have no receipt after {sent} sends: {result:?}"
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn delayed_send_past_partition_lease_must_not_block_an_unrelated_later_turn() {
    let mut h = Harness::new().await;
    let (owner, mut lease) = h.accepted().await;
    h.finish_parent(owner, &mut lease).await;
    let mut capability = h.capability();
    // Membership clocks refresh asynchronously; a capability minted five
    // seconds ago is valid on both clients even across a wall-second boundary.
    capability.issued_at -= 5;
    let key = "slow-receipt";
    let admitted = h.tools[0]
        .reply_delayed(caller(), &capability, key, message())
        .await;
    assert!(
        matches!(admitted, Err(ExecutionError::Busy)),
        "admission: {admitted:?}"
    );
    let id = effect_id(&h, &capability, key);
    let later = later_plan(&h).await;
    let (sink, attempts) = transport(&mut h, Duration::from_secs(16));
    sweep(&h, &id).await;
    sweep(&h, &id).await;
    let result = h.tools[1]
        .reply_delayed(caller(), &capability, key, message())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result.delivery,
        DeliveryOutcome::Confirmed { .. } | DeliveryOutcome::Unknown { .. }
    ));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "receipt loss cannot authorize a resend"
    );
    let mut progressed = false;
    for node in &h.nodes {
        if let Ok(mut next) = node
            .journal()
            .claim(&later, &later.targets[0], "later-worker")
            .await
        {
            progressed = true;
            node.journal().release(&mut next).await.unwrap();
            break;
        }
    }
    sink.abort();
    h.cleanup().await;
    assert!(progressed, "#2171: a 16-second send under a 15-second lease left an unknown effect blocking the next turn");
}

struct Builder(Arc<cmaeg_core::seal::MasterKeyRing>);
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

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn provider_acceptance_after_fifteen_seconds_remains_committable_without_repost() {
    let h = Harness::new().await;
    let (owner, mut lease) = h.acquire().await;
    let journal = h.nodes[owner].journal();
    journal.release(&mut lease).await.unwrap();
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.post_delay = Duration::from_secs(16);
    let port = Arc::new(HttpExecutionPort::new(
        http.client(true, Duration::from_secs(30)),
        Arc::new(Builder(h.database.keys())),
        h.nodes[owner].plans(),
        journal.clone(),
        h.current.clone(),
    ));
    assert!(
        !port
            .support(&h.target().launch.provider)
            .exact_body_idempotency
    );
    let first = port
        .clone()
        .dispatcher()
        .dispatch(&h.plan.identity, &h.target().id, "slow-acceptance")
        .await;
    let accepted = matches!(&first, Ok(DispatchOutcome::Replay { response_id, .. }) if response_id == "resp_accepted");
    let first_error = first.as_ref().err().cloned();
    if let Ok(DispatchOutcome::Replay { mut lease, .. }) = first {
        journal.release(&mut lease).await.unwrap();
    }
    // Placement can change after the second node's membership becomes visible.
    // Recovery must visit both independent clients, like the production scan.
    let (recovery_owner, recovered) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for (index, node) in h.nodes.iter().enumerate() {
                let recovery = Arc::new(HttpExecutionPort::new(
                    http.client(true, Duration::from_secs(30)),
                    Arc::new(Builder(h.database.keys())),
                    node.plans(),
                    node.journal(),
                    h.current.clone(),
                ));
                let result = recovery
                    .dispatcher()
                    .dispatch(&h.plan.identity, &h.target().id, "recovery")
                    .await;
                if !matches!(result, Err(ExecutionError::Busy)) {
                    return (index, result);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("a live replica must recover the expired claim");
    let recoverable = matches!(&recovered, Ok(DispatchOutcome::Replay { response_id, .. }) if response_id == "resp_accepted");
    if let Ok(DispatchOutcome::Replay { mut lease, .. }) = recovered {
        h.nodes[recovery_owner]
            .journal()
            .release(&mut lease)
            .await
            .unwrap();
    }
    let (state, _) = journal
        .read::<TargetState>(&h.target().id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(http.seen().iter().filter(|r| r.method == "POST").count(), 1);
    h.cleanup().await;
    assert!(accepted && recoverable, "#2171: a valid acceptance inside the 20-second budget was discarded: first={first_error:?}, retained phase={:?}", state.phase);
}
