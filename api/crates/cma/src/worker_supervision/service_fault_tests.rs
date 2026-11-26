//! Opt-in whole-service faults. This module is absent from normal CMA binaries.
use super::*;
use std::{collections::HashMap, future::Future, sync::OnceLock, task::Poll};

#[derive(Clone, Copy, Debug)]
pub(super) enum Fault {
    Panic,
    Exit,
    Stall,
    ExitThenStartupStall,
}

fn activities() -> &'static Mutex<HashMap<&'static str, Activity>> {
    static ACTIVITIES: OnceLock<Mutex<HashMap<&'static str, Activity>>> = OnceLock::new();
    ACTIVITIES.get_or_init(Mutex::default)
}

pub(super) async fn inject(activity: Activity, future: impl Future<Output = ()>) {
    let name = activity.snapshot().worker;
    activities().lock().unwrap().insert(name, activity.clone());
    tokio::pin!(future);
    let mut stalled = false;
    std::future::poll_fn(|cx| {
        let fault = activity.0.lock().unwrap().fault.take();
        match fault {
            Some(Fault::Panic) => panic!("injected scheduling panic: {name}"),
            Some(Fault::Exit) => return Poll::Ready(()),
            Some(Fault::ExitThenStartupStall) => {
                activity.0.lock().unwrap().fault = Some(Fault::Stall);
                return Poll::Ready(());
            }
            Some(Fault::Stall) => stalled = true,
            None => {}
        }
        if stalled {
            Poll::Pending
        } else {
            future.as_mut().poll(cx)
        }
    })
    .await
}

struct Probe {
    client: reqwest::Client,
    http: String,
    metrics: String,
}

impl Probe {
    async fn ready(&self) -> Option<(u16, serde_json::Value)> {
        read_ready(&self.client, &self.http).await
    }

    async fn metrics(&self) -> String {
        self.client
            .get(&self.metrics)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    async fn ready_within(&self, seconds: u64) {
        tokio::time::timeout(Duration::from_secs(seconds), async {
            loop {
                if self.ready().await.is_some_and(|(code, _)| code == 200) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("whole service must become ready");
    }
}

async fn read_ready(client: &reqwest::Client, http: &str) -> Option<(u16, serde_json::Value)> {
    let response = client.get(format!("{http}/ready")).send().await.ok()?;
    let code = response.status().as_u16();
    Some((code, response.json().await.ok()?))
}

fn counter(metrics: &str, name: &str, worker: &str, reason: Option<&str>) -> u64 {
    metrics
        .lines()
        .filter(|line| {
            line.starts_with(&format!("{name}{{"))
                && line.contains(&format!("worker=\"{worker}\""))
                && reason.is_none_or(|reason| line.contains(&format!("reason=\"{reason}\"")))
        })
        .map(|line| line.rsplit_once(' ').unwrap().1.parse::<u64>().unwrap())
        .sum()
}

fn fixture_config() -> crate::config::Config {
    assert_eq!(
        std::env::var("CHRONO_TEST_WORKER_SERVICE_FAULTS").as_deref(),
        Ok("isolated")
    );
    let config = crate::config::Config::from_env().unwrap();
    let mongo = url::Url::parse(&config.mongodb_uri).expect("single loopback Mongo URI");
    assert!(matches!(
        mongo.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    ));
    assert!(config
        .mongodb_database
        .starts_with("cma_worker_supervision_"));
    config
}

fn probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "starts the full service; requires explicitly isolated Mongo/provider configuration"]
async fn whole_service_worker_failures_recover_over_http() {
    let config = fixture_config();
    assert!(
        config.bind.ip().is_loopback(),
        "fault tests require loopback HTTP"
    );
    assert!(
        config.metrics_bind.ip().is_loopback(),
        "fault tests require loopback metrics"
    );
    assert!(
        std::env::var_os("KUBECONFIG").is_some()
            && config.runtime.agent_sandbox_connection.is_none(),
        "fault tests require an explicit local Kubernetes fixture"
    );
    let kube = kube::Config::infer()
        .await
        .expect("local Kubernetes fixture");
    assert!(
        matches!(
            kube.cluster_url.host(),
            Some("127.0.0.1" | "localhost" | "[::1]")
        ),
        "fault tests require a loopback Kubernetes endpoint"
    );
    if !config.runtime.sandbox_base_url.is_empty() {
        let provider = url::Url::parse(&config.runtime.sandbox_base_url).unwrap();
        assert!(
            matches!(
                provider.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]")
            ),
            "fault tests require a loopback Sandbox provider"
        );
    }
    let probe = Probe {
        client: probe_client(),
        http: format!("http://{}", config.bind),
        metrics: format!("http://{}/metrics", config.metrics_bind),
    };
    let mut service = tokio::spawn(crate::serve());
    tokio::select! {
        result = &mut service => panic!("whole service exited before readiness: {result:?}"),
        () = probe.ready_within(45) => {}
    }
    let mut results = Vec::new();
    for worker in [
        "launches",
        "agent_commands",
        "sandbox_commands",
        "agents",
        "sandboxes",
        "workspaces",
        "sandbox_reconciliation",
        "agent_capacity_recovery",
        "native_warm_pools",
    ] {
        results.push(exercise(&probe, worker, Fault::Panic).await);
    }
    results.push(exercise(&probe, "agent_commands", Fault::Exit).await);
    results.push(exercise(&probe, "agent_commands", Fault::Stall).await);
    println!(
        "SERVICE_WORKER_FAULT_RESULTS={}",
        serde_json::to_string(&results).unwrap()
    );
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap()
        .success());
    tokio::time::timeout(Duration::from_secs(45), service)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

async fn exercise(probe: &Probe, worker: &'static str, fault: Fault) -> serde_json::Value {
    let before = probe.metrics().await;
    let restarts = counter(&before, "cma_worker_restarts_total", worker, None);
    let reason = match fault {
        Fault::Panic => "panic",
        Fault::Exit => "exit",
        Fault::Stall => "stall",
        Fault::ExitThenStartupStall => unreachable!("startup fault has a dedicated exercise"),
    };
    let exits = counter(
        &before,
        "cma_worker_supervision_exits_total",
        worker,
        Some(reason),
    );
    let started = Instant::now();
    let activity = activities().lock().unwrap().get(worker).unwrap().clone();
    activity.0.lock().unwrap().fault = Some(fault);
    let mut unavailable_at = None;
    let mut recovery_seen = false;
    let mut samples = Vec::new();
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let (code, body) = probe.ready().await.expect("HTTP must keep serving during worker loss");
            assert!(body["dependency"]["reason"].is_null(), "Mongo must stay healthy: {body}");
            let workers = body["workers"].as_array().unwrap();
            let target = workers.iter().find(|entry| entry["worker"] == worker).unwrap();
            for other in workers.iter().filter(|entry| entry["worker"] != worker) {
                assert_eq!(other["state"], "running", "unrelated worker changed: {body}");
            }
            let state = target["state"].as_str().unwrap();
            if state != "running" {
                unavailable_at.get_or_insert(started.elapsed().as_secs_f64());
                if target["required"] == true {
                    assert_eq!(code, 503);
                    assert_eq!(body["reason"], "workers");
                } else {
                    assert_eq!(code, 200, "optional capacity must not fail readiness");
                }
                if state == "backoff" { recovery_seen = true; }
            }
            let live = probe.client.get(format!("{}/livez", probe.http)).send().await.unwrap();
            assert_eq!(live.status().as_u16(), 200);
            samples.push(serde_json::json!({"seconds": started.elapsed().as_secs_f64(), "ready": code, "state": state}));
            if recovery_seen && state == "running" { assert_eq!(code, 200); break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await.expect("worker must fail and recover without restarting the service");
    let recovered_at = started.elapsed().as_secs_f64();
    let after = probe.metrics().await;
    assert_eq!(
        counter(&after, "cma_worker_restarts_total", worker, None),
        restarts + 1
    );
    assert_eq!(
        counter(
            &after,
            "cma_worker_supervision_exits_total",
            worker,
            Some(reason)
        ),
        exits + 1
    );
    for other in [
        "launches",
        "agent_commands",
        "sandbox_commands",
        "agents",
        "sandboxes",
        "workspaces",
        "sandbox_reconciliation",
        "agent_capacity_recovery",
        "native_warm_pools",
    ]
    .into_iter()
    .filter(|name| *name != worker)
    {
        assert_eq!(
            counter(&after, "cma_worker_restarts_total", other, None),
            counter(&before, "cma_worker_restarts_total", other, None)
        );
    }
    let response = probe
        .client
        .get(format!("{}/api/v1/sandboxes", probe.http))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let result = serde_json::json!({"worker":worker,"fault":reason,"unavailable_seconds":unavailable_at,"recovered_seconds":recovered_at,"restart_delta":1,"pid":std::process::id(),"samples":samples});
    println!(
        "worker={worker} fault={reason} unavailable={:.3}s recovered={recovered_at:.3}s restart_delta=1",
        unavailable_at.unwrap()
    );
    result
}

struct MongoReadiness(mongodb::Database);

#[async_trait::async_trait]
impl crate::health::ReadinessProbe for MongoReadiness {
    async fn ping(&self) -> Result<(), layer2_core::StoreError> {
        self.0
            .run_command(mongodb::bson::doc! { "ping": 1 })
            .await
            .map(|_| ())
            .map_err(|error| layer2_core::StoreError::Unavailable(error.to_string()))
    }
}

pub(crate) async fn startup_recovery_with_owned_completion(
    database: mongodb::Database,
    operations: crate::store::MongoOperationStore,
) {
    use layer2_core::{OperationClaim, OperationReservation, OperationStore, SubjectId};
    use std::time::SystemTime;
    use tokio::{
        sync::{mpsc, oneshot},
        task::JoinSet,
    };

    let client = probe_client();
    let claim = OperationClaim::new(
        SubjectId::new("worker-supervision-fixture"),
        "complete-owned-work",
        uuid::Uuid::new_v4().simple().to_string(),
        [1; 32],
        SystemTime::now() + Duration::from_secs(60),
        Duration::from_secs(60),
    );
    let operations = Arc::new(operations);
    assert_eq!(
        operations.reserve(&claim).await.unwrap(),
        OperationReservation::Acquired
    );
    let (release, held) = oneshot::channel();
    let (finished, mut completion) = mpsc::unbounded_channel();
    let mut children = JoinSet::new();
    let child_claim = claim.clone();
    let child_store = operations.clone();
    children.spawn(async move {
        held.await.unwrap();
        child_store
            .complete(&child_claim, b"completed".to_vec())
            .await
            .unwrap();
        Instant::now()
    });
    let workers = Workers::new();
    let activity = workers.get("agent_commands");
    for other in workers
        .snapshot()
        .iter()
        .filter(|worker| worker.worker != "agent_commands")
    {
        let other = workers.get(other.worker);
        other.scheduled();
        other.waiting(Duration::from_secs(30), "idle_scan");
    }
    let (stop, signal) = watch::channel(false);
    let worker = spawn(
        children,
        activity.clone(),
        signal.clone(),
        move |children, activity, shutdown| {
            let finished = finished.clone();
            Box::pin(async move {
                let interval = Duration::from_millis(20);
                loop {
                    activity.waiting(interval, "idle_scan");
                    tokio::select! {
                        _ = crate::shutdown::signalled(shutdown.clone()) => return,
                        _ = tokio::time::sleep(interval) => {}
                        result = children.join_next(), if !children.is_empty() => {
                            finished.send(result.unwrap().unwrap()).unwrap();
                        }
                    }
                    activity.scheduled();
                }
            })
        },
        |children| Box::pin(children.shutdown()),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http = format!("http://{}", listener.local_addr().unwrap());
    let router =
        crate::health::router_with_workers(Arc::new(MongoReadiness(database)), workers, None);
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(crate::shutdown::signalled(signal))
            .await
            .unwrap();
    });
    tokio::time::timeout(LOOP_GRACE, async {
        while read_ready(&client, &http)
            .await
            .is_none_or(|(code, _)| code != 200)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the original scheduler must reach readiness");

    let fault_at = Instant::now();
    activity.0.lock().unwrap().fault = Some(Fault::ExitThenStartupStall);
    let mut saw_startup_stall = false;
    let mut detected_at = None;
    let resumed_at = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let (code, body) = read_ready(&client, &http).await.unwrap();
            assert!(body["dependency"]["reason"].is_null(), "{body}");
            let workers = body["workers"].as_array().unwrap();
            let target = workers
                .iter()
                .find(|worker| worker["worker"] == "agent_commands")
                .unwrap();
            assert!(
                workers
                    .iter()
                    .filter(|worker| worker["worker"] != "agent_commands")
                    .all(|worker| worker["state"] == "running"),
                "{body}"
            );
            let state = target["state"].as_str().unwrap();
            let live = client.get(format!("{http}/livez")).send().await.unwrap();
            assert_eq!(live.status().as_u16(), 200);
            if state == "running" {
                assert_eq!(code, 200);
                if saw_startup_stall {
                    assert!(
                        detected_at.is_some(),
                        "the stalled successor must be restarted"
                    );
                    break fault_at.elapsed();
                }
            } else {
                assert_eq!(code, 503);
                assert_eq!(body["reason"], "workers");
                if state == "starting" && target["waiting"] == "startup" {
                    saw_startup_stall = true;
                }
                if saw_startup_stall && matches!(state, "suspected_stall" | "backoff") {
                    detected_at.get_or_insert(fault_at.elapsed());
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("readiness must recover after restarting the stalled successor");

    assert_eq!(
        operations.reserve(&claim).await.unwrap(),
        OperationReservation::Pending
    );
    release.send(()).unwrap();
    let completed_at = tokio::time::timeout(LOOP_GRACE, completion.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        operations.reserve(&claim).await.unwrap(),
        OperationReservation::Complete(b"completed".to_vec())
    );
    println!(
        "startup successor fault: detection={:?}, scheduling={resumed_at:?}, durable completion={:?}",
        detected_at.unwrap(),
        completed_at.duration_since(fault_at),
    );
    stop.send(true).unwrap();
    worker.await.unwrap();
    server.await.unwrap();
    assert_eq!(completion.recv().await, None, "owned work completes once");
}
