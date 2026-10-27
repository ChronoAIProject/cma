//! Signed native webhook through real routed provider HTTP and native reply receipts.
use crate::whatsapp::{
    tests::{credentials, envelope, reaction, source, text},
    webhook, Client, Transport,
};
use crate::{
    channels::resolver::ChannelResolver,
    delivery::{
        custom::CustomDelivery, durable::DeliveryCoordinator, engine::DeliveryEngine,
        relay_client::RelayClient, router::ShardedScheduler,
    },
    http::readiness::Readiness,
    ingress::{
        launch::UnavailableLauncher,
        routed::RoutedIngress,
        routing_plan::fixture,
        routing_store::{
            test_server::{Row, Server},
            PlanStore,
        },
        IngressState,
    },
    net::exchange::{HttpReply, TransportError},
    turns::{
        routed::{
            adapter_test_server::HttpServer,
            scheduler::{ScheduleConfig, Scheduler},
            Authority, HttpExecutionPort, Journal, Phase, ProductionFrozenTurnBuilder,
            RoutedDelivery, RoutedTargetExecutor, TargetState,
        },
        TurnTracker,
    },
    DeliveryConfig,
};
use cmaeg_core::seal::{seal_blob, ChannelPointer};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

struct Graph(Mutex<Vec<Value>>);
impl Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, Method::POST);
        assert_eq!(url.path(), "/v25.0/34567/messages");
        let mut calls = self.0.lock().unwrap();
        calls.push(serde_json::from_slice(&body).unwrap());
        let id = format!("wamid.sent-{}", calls.len());
        Box::pin(async move {
            Ok(HttpReply::new(StatusCode::OK, serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":id}]})).unwrap()))
        })
    }
}
fn frames() -> Vec<Value> {
    let lifecycle = |sequence, kind, status| json!({"type":kind,"sequence_number":sequence,"response":{"id":"resp_accepted","object":"response","status":status,"conversation":{"id":"agt_native"},"metadata":{},"output":[]}});
    vec![
        lifecycle(0, "response.created", "queued"),
        json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"m","delta":"native response"}),
        lifecycle(2, "response.completed", "completed"),
    ]
}
async fn post(url: &str, message: Value) {
    let body = envelope(vec![message]);
    let secret = credentials();
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(
            ring::hmac::HMAC_SHA256,
            secret.app_secret.as_str().as_bytes(),
        ),
        &body,
    );
    let response = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header(
            "x-hub-signature-256",
            format!("sha256={}", webhook::hex(tag.as_ref())),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"accepted":1,"refused":0})
    );
}
async fn complete(store: &PlanStore, journal: &Journal, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut done = 0;
            let mut after = None;
            loop {
                let page = store.recover_all_page(after.as_deref(), 32).await.unwrap();
                for plan in page.plans {
                    for target in plan.targets {
                        if journal
                            .read::<TargetState>(&target.id)
                            .await
                            .unwrap()
                            .is_some_and(|(state, _)| {
                                matches!(state.phase, Phase::Completed { .. })
                            })
                        {
                            done += 1;
                        }
                    }
                }
                after = page.next_cursor;
                if after.is_none() {
                    break;
                }
            }
            if done == count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("native provider turns must complete");
}

#[tokio::test]
async fn native_whatsapp_real_provider_retries_run_once_and_outbound_reactions_reply_natively() {
    let records = Server::start_realtime().await;
    let keys = Arc::new(fixture::keys());
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let request = fixture::request(
        |value| {
            value["record_id"] = uuid::Uuid::new_v4().to_string().into();
            value["sources"] = json!([source()]);
            value["routing"]["consumers"] = json!([]);
            value["routing"]["default_targets"] = json!(["default"]);
            value["secrets"]["consumer_keys"] = json!([]);
            value["reply"]["sinks"] = json!([{"type":"source"}]);
            value["reply"]["auto_final"] = true.into();
            let secret = credentials();
            value["secrets"]["whatsapp"] = json!({"app_secret":secret.app_secret.as_str(),"access_token":secret.access_token.as_str(),"verify_token":secret.verify_token.as_str()});
        },
        fixture::message("unused"),
    );
    let definition = &request.channel.definition;
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: records
            .records
            .record_url(definition.record_id.as_str())
            .unwrap()
            .to_string(),
    };
    let channel = pointer.seal_id(&keys).unwrap();
    records.memory.channels.lock().unwrap().insert(
        definition.record_id.to_string(),
        Row {
            owner: definition.owner.subject.clone(),
            blob: seal_blob(
                &keys,
                &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
            )
            .unwrap(),
            expiry: now + 3600,
            version: 1,
        },
    );
    let resolver = ChannelResolver::new(
        Arc::new(records.records.clone()),
        keys.clone(),
        Default::default(),
    );
    let plans = PlanStore::new(records.records.clone(), keys.clone());
    let journal = Journal::new(records.records.clone(), keys.clone());
    let provider = HttpServer::start().await;
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.replay_frames = Some(frames());
        memory.config.mirror_replay_metadata = true;
    }
    let providers = provider.client(true, Duration::from_secs(5));
    let authority: Arc<dyn Authority> = Arc::new(resolver.clone());
    let port = Arc::new(HttpExecutionPort::new(
        providers.clone(),
        Arc::new(ProductionFrozenTurnBuilder::new(providers, keys.clone())),
        plans.clone(),
        journal.clone(),
        authority.clone(),
    ));
    let tracker = TurnTracker::new(Readiness::initialized());
    let (tx, rx) = mpsc::channel(8);
    let sink = Arc::new(ShardedScheduler::new(tx));
    let graph = Arc::new(Graph(Mutex::default()));
    let receipts = Arc::new(crate::whatsapp::receipts::Store::new(
        records.records.clone(),
        keys.clone(),
    ));
    let relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
        .unwrap()
        .with_whatsapp(
            Client::production().unwrap().with_transport(graph.clone()),
            receipts.clone(),
        );
    let engine = DeliveryEngine::start_observed(
        rx,
        keys.clone(),
        relay,
        DeliveryConfig::default(),
        tracker.clone(),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
        Arc::new(crate::turns::observer::NoopObserver),
    );
    let delivery = RoutedDelivery::new(
        plans.clone(),
        journal.clone(),
        authority,
        DeliveryCoordinator::new(sink),
    );
    let executor = Arc::new(RoutedTargetExecutor::new(
        port,
        delivery,
        tracker,
        &DeliveryConfig::default(),
    ));
    let (scheduler, schedule) = Scheduler::new(
        Arc::new(plans.clone()),
        executor,
        ScheduleConfig {
            sweep_delay: Duration::from_millis(20),
            drain_timeout: Duration::from_secs(5),
            ..Default::default()
        },
    )
    .unwrap();
    let routed = RoutedIngress::new(
        plans.clone(),
        schedule,
        keys.clone(),
        BTreeSet::new(),
        BTreeSet::from(["cma_codex".into()]),
        BTreeSet::from(["cma_codex".into()]),
    );
    let ingress = IngressState::new(
        resolver,
        keys,
        Default::default(),
        Arc::new(UnavailableLauncher),
    )
    .unwrap()
    .with_routed(Some(Arc::new(routed)))
    .with_whatsapp_receipts(receipts);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/whatsapp/{channel}",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::http::whatsapp::router(ingress))
            .await
            .unwrap();
    });
    let (stop, stopped) = watch::channel(false);
    let worker = tokio::spawn(scheduler.run(stopped));
    let mut message = text("wamid.e2e", "hello");
    message["timestamp"] = now.to_string().into();
    post(&url, message.clone()).await;
    post(&url, message).await;
    complete(&plans, &journal, 1).await;
    let mut message = reaction("wamid.e2e-reaction", "wamid.sent-1", "👍");
    message["timestamp"] = now.to_string().into();
    post(&url, message.clone()).await;
    post(&url, message).await;
    complete(&plans, &journal, 2).await;
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
    server.abort();
    let calls = graph.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["context"]["message_id"], "wamid.e2e");
    assert_eq!(calls[1]["context"]["message_id"], "wamid.sent-1");
    for call in calls.iter() {
        assert_eq!(call["text"]["body"], "native response");
        assert_eq!(call["to"], "447700900123");
    }
    let seen = provider.seen();
    let turns: Vec<_> = seen
        .iter()
        .filter(|call| call.method == "POST" && call.uri == "/api/v2/responses")
        .collect();
    assert_eq!(turns.len(), 2);
    for turn in turns {
        let body: Value = serde_json::from_slice(&turn.body).unwrap();
        assert_eq!(
            body["event_context"]["activity"]["actor"]["kind"],
            "unknown"
        );
        for secret in [
            "fixture-app-secret-12345",
            "fixture-access-token-12345",
            "fixture-verify-token-12345",
        ] {
            assert!(!body.to_string().contains(secret));
        }
    }
}
