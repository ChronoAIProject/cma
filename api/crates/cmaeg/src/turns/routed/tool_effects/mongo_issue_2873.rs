use super::{super::replica_test_fixture::*, mongo_progress_fixture::*, *};
use crate::{
    delivery::{custom::CustomDelivery, engine::DeliveryEngine, relay_client::RelayClient},
    http::readiness::Readiness,
    records::RecordClient,
    replicas::{delayed_lane, effect_recovery::Recovery, partition_discovery::Frontier},
    tools::{fake_cma::FakeCma, reply_targets::native_fixture::native_state},
    turns::{
        routed::{
            adapter_test_server::HttpServer,
            scheduler::{ScheduleConfig, Scheduler},
            HttpExecutionPort, RoutedDelivery, RoutedTargetExecutor,
        },
        TurnTracker,
    },
    DeliveryConfig,
};
use axum::{routing::post, Json, Router};
use cmaeg_core::{
    activity::{ConversationKind, SourceKind},
    channel::{Platform, Secret},
    seal::{ReplyAddress, ReplyTargetRef},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{
    sync::{watch, Notify},
    time::timeout,
};

const BOUND: Duration = Duration::from_secs(10);

async fn pending_http(response: reqwest::Response) {
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    assert_eq!(response.headers()["retry-after"], "2");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["detail"], "routed_reply_in_progress");
}

async fn gateway_http(router: Router) -> (String, Task<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (
        origin,
        Task::new(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        })),
    )
}

async fn same_lark_chat(legacy: bool) {
    let auth_key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
    let http_caller = AgentCaller {
        key_hash: Sha256::digest(auth_key.as_bytes()).into(),
    };
    let backend = FakeCma::start(false).await;
    let mut activity = crate::ingress::routing_plan::fixture::message("synthetic A");
    activity.conversation.kind = ConversationKind::Group;
    activity.source = SourceKind::NyxidRelay {
        platform: Platform::Lark,
        route_id: crate::ingress::routing_plan::fixture::ROUTE.into(),
    };
    let mut request = crate::ingress::routing_plan::fixture::request(
        |value| {
            value["record_id"] = json!(uuid::Uuid::new_v4().to_string());
            value["routing"]["default_targets"] = json!(["default"]);
            value["sources"][1] = crate::channels::routing_fixture::relay(
                "lark",
                crate::ingress::routing_plan::fixture::ROUTE,
            );
            value["secrets"]["channel_key"] = json!("fixture-channel-key");
            value["secrets"]["agent_key_hash"] =
                json!(crate::channels::definition::hash_secret(&auth_key));
        },
        activity,
    );
    crate::ingress::routing_plan::fixture::rewrite_ref(&mut request, |event| {
        let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut event.reply else {
            panic!("Lark route required");
        };
        *thread_reply = true;
    });
    let h = Harness::with_http_request(request, false, Some((&backend, http_caller))).await;
    run_harness(h, |h| Box::pin(async move {
        let (owner, mut a) = h.accepted().await;
        let partition = h.nodes[owner].head_binding(&h.plan, h.target()).await.partition;
        h.finish_parent(owner, &mut a).await;
        if legacy {
            h.nodes[1].lane_capability(false).await;
        }
        let b = later_plan(h).await;
        let target = &b.targets[0];
        assert_ne!(h.plan.identity.event_id, b.identity.event_id);
        assert_eq!(h.plan.identity.channel_id, b.identity.channel_id);
        assert_eq!(h.target().launch.consumer, target.launch.consumer);
        assert_eq!(h.plan.activity.conversation.id, b.activity.conversation.id);
        assert_eq!(h.nodes[0].head_binding(&b, target).await.partition, partition);

        let provider = HttpServer::start().await;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        provider.memory.lock().unwrap().config.replay_barrier = Some((entered.clone(), release.clone()));
        let sends = Arc::new(AtomicUsize::new(0));
        let counted = sends.clone();
        let api = Router::new().route("/api/v1/channel-relay/thread-reply", post(move |Json(body): Json<Value>| {
            let counted = counted.clone();
            async move {
                assert_eq!(body["message_id"], "incoming-fixture");
                assert_eq!(body["thread_reply"], true);
                counted.fetch_add(1, Ordering::SeqCst);
                Json(json!({"message_id":"fixture-out","platform_message_id":"fixture-platform"}))
            }
        }));
        let (origin, _server) = gateway_http(api).await;
        let tracker = TurnTracker::new(Readiness::initialized());
        let config = DeliveryConfig::default();
        let engine = DeliveryEngine::start(
            std::mem::replace(&mut h.rx, tokio::sync::mpsc::channel(1).1),
            h.database.keys(),
            RelayClient::new(&origin, Duration::from_secs(2)).unwrap(),
            config.clone(),
            tracker.clone(),
            CustomDelivery::new(origin, Default::default()),
        );
        let (stop, stopped) = watch::channel(false);
        let mut tasks = Vec::new();
        for (node, tools) in h.nodes.iter().zip(&h.tools) {
            let journal = node.journal();
            let port = Arc::new(HttpExecutionPort::new(
                provider.client(true, Duration::from_secs(30)),
                Arc::new(Builder(h.database.keys())),
                node.plans(),
                journal.clone(),
                h.current.clone(),
            ));
            let delivery = RoutedDelivery::new(node.plans(), journal, h.current.clone(), tools.coordinator.clone());
            let turns = Arc::new(RoutedTargetExecutor::new(port, delivery, tracker.clone(), &config));
            let effects = tools.replica_effect_executor();
            let executor = node.journal().effect_executor(node.plans(), turns, effects.clone()).unwrap();
            let source = Arc::new(Frontier { partitions: node.partitions.clone(), plans: node.plans() });
            let (scheduler, _) = Scheduler::new(Arc::new(Recovery::new(source, effects)), executor,
                ScheduleConfig { sweep_delay: Duration::from_millis(100), ..Default::default() }).unwrap();
            tasks.push(Task::new(tokio::spawn(scheduler.run(stopped.clone()))));
        }
        timeout(BOUND, entered.notified()).await.expect("B must reach the live provider replay");
        assert_eq!(provider.seen().iter().filter(|r| r.method == "POST").count(), 1);
        assert!(matches!(h.nodes[0].journal().read::<TargetState>(&target.id).await.unwrap().unwrap().0.phase, Phase::Accepted { .. }));
        let mut capability = h.capability();
        capability.issued_at -= 5;
        capability.agent_key_hash = http_caller.key_hash;
        let ReplyAddress::NyxidRelay { reply_token, reply_token_expires_at, .. } = &mut capability.event.reply else {
            panic!("Lark route required");
        };
        *reply_token = Secret::new("");
        *reply_token_expires_at = 1;
        let mut frozen = message();
        frozen.thread_reply = true;
        let idempotency_key = "reply";
        let payload = Payload { capability: capability.clone(), caller: http_caller.key_hash, message: frozen.clone() };
        let id = model::work_id(&model::operation(h.target(), &payload, idempotency_key).unwrap());
        let reference = ReplyTargetRef::seal(&capability, &h.database.keys()).unwrap();
        let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
        let mut gateways = Vec::new();
        for tools in &h.tools {
            let mut state = native_state::state(records.clone(), h.database.keys(), &provider);
            state.routed_replies = Some(tools.clone());
            gateways.push(gateway_http(crate::http::tools::router(state)).await);
        }
        let client = reqwest::Client::builder().no_proxy().timeout(BOUND).build().unwrap();
        let body = serde_json::to_vec(&frozen).unwrap();
        let request = |node: usize| client.post(format!(
            "{}/v1/reply-targets/{}/messages", gateways[node].0, reference.as_str()))
            .bearer_auth(&auth_key)
            .header("Idempotency-Key", idempotency_key)
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send();
        let admitted = tokio::time::Instant::now();
        let mut response = request(0).await.unwrap();
        let deployment = h.nodes[0].partitions.membership().deployment;
        let expected_queue = if legacy { partition.clone() } else { delayed_lane::id(&deployment, &h.plan, h.target()) };
        assert_eq!(h.nodes[0].custody(&id).await.unwrap().0, expected_queue);
        if legacy {
            pending_http(response).await;
            h.nodes[1].lane_capability(true).await;
            pending_http(request(1).await.unwrap()).await;
            assert_eq!(sends.load(Ordering::SeqCst), 0);
            assert_eq!(h.nodes[0].custody(&id).await.unwrap().0, partition);
            assert_eq!(h.nodes[0].head_binding(&b, target).await.head_work, target.id);
            release.notify_one();
            response = request(0).await.unwrap();
        }
        let first = timeout(BOUND, async {
            while response.status() != reqwest::StatusCode::OK {
                pending_http(response).await;
                response = request(0).await.unwrap();
            }
            response.json::<Value>().await.unwrap()
        }).await.expect("A reply must complete within bound");
        let replay = request(1).await.unwrap();
        assert_eq!(replay.status(), reqwest::StatusCode::OK);
        assert_eq!(first["delivery"]["outcome"], "confirmed");
        assert_eq!(first["delivery"]["platform_message_id"], "fixture-platform");
        assert_eq!(first, replay.json::<Value>().await.unwrap());
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        println!("issue2873_http custody={} actual_posts=1 same_receipt=true elapsed_ms={}", if legacy {"legacy_fifo"} else {"delayed_lane"}, admitted.elapsed().as_millis());
        if !legacy {
            assert!(admitted.elapsed() < BOUND);
            assert!(matches!(h.nodes[1].journal().read::<TargetState>(&target.id).await.unwrap().unwrap().0.phase, Phase::Accepted { .. }));
            release.notify_one();
        }
        timeout(BOUND, async {
            loop {
                let b = h.nodes[1].journal().read::<TargetState>(&target.id).await.unwrap().unwrap();
                if matches!(b.0.phase, Phase::Completed { .. }) && h.nodes[1].custody(&id).await.unwrap().1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("B and reply queue item must settle");
        stop.send(true).unwrap();
        for task in tasks {
            let report = timeout(BOUND, task.join()).await.unwrap();
            assert!(!report.recovery_failed);
            assert_eq!(report.aborted, 0);
        }
        assert!(engine.drain(std::time::Instant::now() + BOUND).await);
    })).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn issue_2873_current_lane_posts_while_b_executes() {
    same_lark_chat(false).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn issue_2873_legacy_fifo_waits_for_b() {
    same_lark_chat(true).await;
}
