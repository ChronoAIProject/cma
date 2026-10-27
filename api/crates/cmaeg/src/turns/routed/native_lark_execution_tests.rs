//! Signed native webhook through real routed provider HTTP and native reply receipts.
use crate::native_lark::{
    test_support::{credentials, reaction, recall, signed, source, text},
    Client, Transport,
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
use tokio::sync::{mpsc, watch, Notify, Semaphore};

struct Graph {
    calls: Mutex<Vec<Value>>,
    first_seen: Notify,
    release_first: Semaphore,
}
impl Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, Method::POST);
        assert_eq!(url.host_str(), Some("open.feishu.cn"));
        if url.path() == "/open-apis/auth/v3/tenant_access_token/internal" {
            return Box::pin(async {
                Ok(HttpReply::new(StatusCode::OK,serde_json::to_vec(&json!({"code":0,"tenant_access_token":"fixture-native-access-token","expire":7200})).unwrap()))
            });
        }
        assert_eq!(url.path(), "/open-apis/im/v1/messages");
        assert_eq!(url.query(), Some("receive_id_type=chat_id"));
        let number = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(serde_json::from_slice(&body).unwrap());
            calls.len()
        };
        let id = format!("om_sent_{number}");
        Box::pin(async move {
            if number == 1 {
                self.first_seen.notify_one();
                self.release_first.acquire().await.unwrap().forget();
            }
            Ok(HttpReply::new(
                StatusCode::OK,
                serde_json::to_vec(
                    &json!({"code":0,"data":{"message_id":id,"chat_id":"oc_native_chat"}}),
                )
                .unwrap(),
            ))
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
async fn post(url: &str, message: Value, expected: StatusCode) {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let (body, signature) = signed(&message, now);
    let response = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header("x-lark-request-timestamp", now.to_string())
        .header("x-lark-request-nonce", "fixture_nonce")
        .header("x-lark-signature", signature)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), expected);
    if expected == StatusCode::OK {
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"accepted":true})
        );
    }
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
async fn native_lark_signed_ingress_retries_receipts_recall_and_reaction_run_once() {
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
            value["secrets"]["native_lark"] = json!({"app_secret":secret.app_secret.as_str(),"encrypt_key":secret.encrypt_key.as_str(),"verification_token":secret.verification_token.as_str()});
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
    let graph = Arc::new(Graph {
        calls: Mutex::default(),
        first_seen: Notify::new(),
        release_first: Semaphore::new(0),
    });
    let receipts = Arc::new(crate::native_lark::receipts::Store::new(
        records.records.clone(),
        keys.clone(),
    ));
    let relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
        .unwrap()
        .with_native_lark(
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
    .with_native_lark_intake(true);
    let disabled = ingress.clone().with_native_lark_intake(false);
    let disabled_response = crate::http::native_lark::callback(
        axum::extract::State(disabled),
        axum::extract::Path(channel.to_string()),
        axum::http::Request::new(axum::body::Body::empty()),
    )
    .await;
    assert_eq!(disabled_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let restarted = IngressState::new(
        ChannelResolver::new(
            Arc::new(records.records.clone()),
            ingress.master_keys.clone(),
            Default::default(),
        ),
        ingress.master_keys.clone(),
        Default::default(),
        Arc::new(UnavailableLauncher),
    )
    .unwrap()
    .with_routed(ingress.routed.clone())
    .with_native_lark_intake(true);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut url = format!(
        "http://{}/v1/native-lark/{channel}",
        listener.local_addr().unwrap()
    );
    let mut server = tokio::spawn(async move {
        axum::serve(listener, crate::http::native_lark::router(ingress))
            .await
            .unwrap();
    });
    let (stop, stopped) = watch::channel(false);
    let worker = tokio::spawn(scheduler.run(stopped));
    let client = reqwest::Client::new();
    let challenge = json!({"type":"url_verification","token":credentials().verification_token.as_str(),"challenge":"authenticated-challenge"});
    let (body, signature) = signed(&challenge, now);
    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .header("x-lark-request-timestamp", now.to_string())
        .header("x-lark-request-nonce", "fixture_nonce")
        .header("x-lark-signature", &signature)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"challenge":"authenticated-challenge"})
    );
    let bad = client
        .post(&url)
        .header("content-type", "application/json")
        .header("x-lark-request-timestamp", now.to_string())
        .header("x-lark-request-nonce", "fixture_nonce")
        .header("x-lark-signature", "0".repeat(64))
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    assert!(plans
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans
        .is_empty());
    let original = text("om_e2e", "hello", now);
    post(&url, original.clone(), StatusCode::OK).await;
    post(&url, original, StatusCode::OK).await;
    tokio::time::timeout(Duration::from_secs(5), graph.first_seen.notified())
        .await
        .unwrap();
    let delta = reaction("event_reaction", "om_sent_1", true, now);
    post(&url, delta.clone(), StatusCode::SERVICE_UNAVAILABLE).await;
    graph.release_first.add_permits(1);
    complete(&plans, &journal, 1).await;
    // A fresh receiver has no prior in-memory original or receipt context.
    server.abort();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    url = format!(
        "http://{}/v1/native-lark/{channel}",
        listener.local_addr().unwrap()
    );
    server = tokio::spawn(async move {
        axum::serve(listener, crate::http::native_lark::router(restarted))
            .await
            .unwrap();
    });

    post(&url, delta.clone(), StatusCode::OK).await;
    post(&url, delta, StatusCode::OK).await;
    complete(&plans, &journal, 2).await;
    let recalled = recall("event_recall", "om_e2e", now);
    post(&url, recalled.clone(), StatusCode::OK).await;
    post(&url, recalled, StatusCode::OK).await;
    complete(&plans, &journal, 3).await;
    let removed = reaction("event_remove", "om_sent_2", false, now);
    post(&url, removed.clone(), StatusCode::OK).await;
    post(&url, removed, StatusCode::OK).await;
    complete(&plans, &journal, 4).await;
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
    server.abort();
    let calls = graph.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    for call in calls.iter() {
        assert_eq!(call["receive_id"], "oc_native_chat");
        assert_eq!(call["msg_type"], "text");
        assert!(call["content"]
            .as_str()
            .unwrap()
            .contains("native response"));
    }
    let seen = provider.seen();
    let turns: Vec<_> = seen
        .iter()
        .filter(|call| call.method == "POST" && call.uri == "/api/v2/responses")
        .collect();
    assert_eq!(turns.len(), 4);
    for turn in turns {
        let body: Value = serde_json::from_slice(&turn.body).unwrap();
        let activity = &body["event_context"]["activity"];
        let recalled = activity["kind"]["ce"]["type"] == "cma.message.deleted.v1";
        assert_eq!(
            activity["actor"]["kind"],
            if recalled { "unknown" } else { "human" }
        );
        if recalled {
            assert_eq!(activity["actor"]["id"], "unknown");
            assert_eq!(
                activity["kind"]["ce"]["data"]["recall_type"],
                "group_manager"
            );
        }
        for secret in [
            "fixture-native-app-secret-12345",
            "fixture-native-encrypt-key-12345",
            "fixture-native-verification-12345",
        ] {
            assert!(!body.to_string().contains(secret));
        }
    }
}
