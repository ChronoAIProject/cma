//! Reconcile every response outcome before reporting the first invariant failure.
use super::{
    delivery::{runner_keys, DeliveryOracle},
    ingress_reason,
    queue::QueueOracle,
    trace::Trace,
};
use crate::{
    runner_process::{capture::Ledger, database::DatabaseFixture, Process},
    support::{Envelope, Fixture},
};
use cmaeg::ingress::routing_store::EventIdentity;
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::{
    panic::AssertUnwindSafe,
    time::{Duration, Instant},
};

pub struct Workload<'a> {
    pub fixture: &'a Fixture,
    pub processes: &'a mut [Process],
    pub database: &'a DatabaseFixture,
    pub identity: &'a EventIdentity,
    pub envelope: &'a Envelope,
    pub enabled: bool,
    pub trace: &'a Trace,
    pub ledger: &'a mut Ledger,
}

impl Workload<'_> {
    pub async fn run(&mut self) -> Vec<&'static str> {
        let mut failures = Vec::new();
        let (body, token) = self.envelope.signed();
        let requests = self.processes.iter().enumerate().map(|(index, process)| {
            let mut request = self.fixture.request(body.clone(), &token).build().unwrap();
            *request.url_mut() = format!("{}/v1/callbacks/{}", process.base, self.fixture.id).parse().unwrap();
            let client = &self.fixture.client;
            let trace = self.trace;
            let ledger = &*self.ledger;
            async move {
                ledger.record(json!({"stage":"request_start", "ordinal":index+1}));
                let started = Instant::now();
                trace.stage("ingress", "start", index + 1);
                trace.stage("headers", "start", index + 1);
                let request = async {
                    let response = client.execute(request).await.map_err(|_| "transport")?;
                    let status = response.status().as_u16();
                    let headers_us = started.elapsed().as_micros() as u64;
                    ledger.record(json!({"stage":"headers", "ordinal":index+1, "status":status, "headers_us":headers_us}));
                    trace.headers(index + 1, status);
                    trace.stage("headers", "end", index + 1);
                    trace.stage("body", "start", index + 1);
                    let body = response.json::<Value>().await;
                    trace.stage("body", "end", index + 1);
                    let body_us = started.elapsed().as_micros() as u64;
                    Ok::<_, &str>((status, headers_us, body_us, body))
                };
                let result = tokio::time::timeout(Duration::from_secs(25), request).await;
                trace.stage("ingress", "end", index + 1);
                (index + 1, result)
            }
        });
        self.ledger.record(
            json!({"stage":"requests_start", "attempts":self.processes.len(), "unique_events":1}),
        );
        let mut accepted = 0;
        for (ordinal, result) in futures_util::future::join_all(requests).await {
            match result {
                Ok(Ok((status, headers_us, body_us, body))) => {
                    let reason = body.as_ref().map_or("invalid_body", ingress_reason);
                    let code = body.as_ref().ok().and_then(|body| {
                        serde_json::from_value::<cmaeg_core::problem::ProblemCode>(
                            body["code"].clone(),
                        )
                        .ok()
                    });
                    let valid = status == 202
                        && body.as_ref().is_ok_and(|body| body["status"] == "accepted");
                    accepted += usize::from(valid);
                    self.ledger.record(
                        json!({"stage":"response", "ordinal":ordinal, "status":status,
                        "reason":reason, "public_code":code, "body_valid":body.is_ok(), "accepted":valid,
                        "headers_us":headers_us, "body_us":body_us}),
                    );
                    if !valid {
                        failures.push("cross_ingress_not_accepted");
                    }
                    if status != 202 {
                        eprintln!(
                            "replica_rejection replicas={} ownership={} ingress_rejection={reason}",
                            self.processes.len(),
                            self.enabled
                        );
                    }
                }
                error => {
                    let classification = if error.is_err() {
                        "request_timeout"
                    } else {
                        "transport"
                    };
                    self.ledger.record(json!({"stage":"response", "ordinal":ordinal, "status":null, "failure":classification, "accepted":false}));
                    failures.push(classification);
                }
            }
        }
        self.ledger.record(json!({"stage":"oracle_start", "accepted":accepted,
            "durable_plans":null, "targets":null, "confirmed_targets":null, "completed_queue_items":null}));
        self.trace.stage("oracle", "start", 0);
        let oracle = AssertUnwindSafe(async {
            let oracle = DeliveryOracle::open(&self.database.uri, &self.database.name, runner_keys()).await;
            let queue = QueueOracle::open(&self.database.uri, &self.database.name, runner_keys()).await;
            loop {
                let plan = oracle.plan(self.identity).await;
                if let Some(plan) = plan {
                    let targets = oracle.targets(&plan).await;
                    let confirmed = targets.values().filter(|value| value.as_ref().is_some_and(|value| value.confirmed_terminal_text())).count();
                    self.ledger.record(json!({"stage":"oracle_targets", "durable_plans":1,
                        "targets":targets.len(), "confirmed_targets":confirmed, "completed_queue_items":null}));
                    let completed = if self.enabled { Some(queue.completed(&plan).await) } else { None };
                    self.ledger.record(json!({"stage":"oracle_queue", "completed_queue_items":completed}));
                    if plan.targets.len() == 1 && confirmed == 1 && (!self.enabled || completed == Some(1)) { break; }
                } else {
                    self.ledger.record(json!({"stage":"oracle_targets", "durable_plans":0,
                        "targets":null, "confirmed_targets":null, "completed_queue_items":null}));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }).catch_unwind();
        let terminal = tokio::time::timeout(Duration::from_secs(20), oracle).await;
        let outcome = match terminal {
            Ok(Ok(())) => "complete",
            Ok(Err(_)) => "oracle_failure",
            Err(_) => "oracle_timeout",
        };
        self.ledger
            .record(json!({"stage":"oracle_end", "outcome":outcome}));
        self.trace.stage("oracle", "end", 0);
        if outcome != "complete" {
            failures.push(outcome);
        }
        failures
    }
}

pub async fn stop(
    processes: &mut [Process],
    trace: &Trace,
    ledger: &mut Ledger,
) -> Vec<&'static str> {
    let mut failures = Vec::new();
    for (index, process) in processes.iter_mut().enumerate() {
        trace.stage("exit", "start", index + 1);
        ledger.record(json!({"stage":"exit_start", "ordinal":index+1, "joined":false}));
        let join = AssertUnwindSafe(async {
            process.terminate().await;
            process.wait().await
        })
        .catch_unwind();
        let result = tokio::time::timeout(Duration::from_secs(20), join).await;
        let joined = matches!(&result, Ok(Ok(Ok(_))));
        let outcome = match result {
            Ok(Ok(Ok(status))) if status.success() => "joined",
            Ok(Ok(Ok(_))) => "child_failure",
            Ok(Ok(Err(_))) => "join_failure",
            Ok(Err(_)) => "signal_failure",
            Err(_) => "exit_timeout",
        };
        ledger.record(
            json!({"stage":"exit_end", "ordinal":index+1, "outcome":outcome, "joined":joined}),
        );
        if joined {
            if process.mark_joined().is_err() {
                failures.push("metadata_write");
            }
            trace.stage("exit", "end", index + 1);
        }
        if outcome != "joined" {
            failures.push(outcome);
        }
        let safe_logs =
            std::panic::catch_unwind(AssertUnwindSafe(|| process.assert_safe_logs())).is_ok();
        ledger.record(json!({"stage":"safe_logs", "ordinal":index+1, "passed":safe_logs}));
        if !safe_logs {
            failures.push("unsafe_trace");
        }
    }
    failures
}
