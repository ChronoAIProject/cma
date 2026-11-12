//! Opt-in whole-service faults. This module is absent from normal CMA binaries.
use super::*;
use std::{collections::HashMap, future::Future, sync::OnceLock, task::Poll};

#[derive(Clone, Copy, Debug)]
pub(super) enum Fault {
    Panic,
    Exit,
    Stall,
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
        let response = self
            .client
            .get(format!("{}/ready", self.http))
            .send()
            .await
            .ok()?;
        let code = response.status().as_u16();
        Some((code, response.json().await.ok()?))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "starts the full service; requires explicitly isolated Mongo/provider configuration"]
async fn whole_service_worker_failures_recover_over_http() {
    assert_eq!(
        std::env::var("CHRONO_TEST_WORKER_SERVICE_FAULTS").as_deref(),
        Ok("isolated")
    );
    let config = crate::Config::from_env().unwrap();
    let mongo = url::Url::parse(&config.mongodb_uri).expect("single loopback Mongo URI");
    assert!(matches!(
        mongo.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    ));
    assert!(config
        .mongodb_database
        .starts_with("cma_worker_supervision_"));
    assert!(
        config.bind.ip().is_loopback(),
        "fault tests require loopback HTTP"
    );
    assert!(
        config.metrics_bind.ip().is_loopback(),
        "fault tests require loopback metrics"
    );
    let probe = Probe {
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap(),
        http: format!("http://{}", config.bind),
        metrics: format!("http://{}/metrics", config.metrics_bind),
    };
    let service = tokio::spawn(crate::serve());
    probe.ready_within(45).await;
    let mut results = Vec::new();
    for worker in [
        "launches",
        "agent_commands",
        "sandbox_commands",
        "agents",
        "sandboxes",
        "workspaces",
        "sandbox_reconciliation",
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
