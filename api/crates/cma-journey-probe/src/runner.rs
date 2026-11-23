use std::{
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::{
    net::TcpListener,
    task::JoinSet,
    time::{self, Instant},
};

use crate::{
    config::Config,
    ledger::{Claim, Ledger, Recovery},
    model::{now, Intent, Mode, Observation, Phase, Reason},
    monitor::Monitor,
    protocol::{Outcome, Protocol},
};

pub async fn serve(config: Arc<Config>, ledger: Ledger) -> Result<(), &'static str> {
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|_| "cannot bind monitoring listener")?;
    let protocol = Arc::new(Protocol::new(config.clone())?);
    let monitor = Monitor {
        config,
        ledger,
        heartbeat: Arc::new(AtomicI64::new(0)),
        evidence: Arc::new(AtomicBool::new(false)),
        cleanup_failure: Arc::new(AtomicI64::new(0)),
    };
    let scheduler = tokio::spawn(run(monitor.clone(), protocol));
    let result = axum::serve(listener, monitor.routes())
        .with_graceful_shutdown(async {
            if tokio::signal::ctrl_c().await.is_err() {
                tracing::error!("shutdown signal listener failed");
            }
        })
        .await;
    scheduler.abort();
    result.map_err(|_| "monitoring listener failed")
}

async fn run(monitor: Monitor, protocol: Arc<Protocol>) {
    let mut interval = time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut jobs = JoinSet::new();
    let mut configured = false;
    let mut prune_at = 0;
    loop {
        interval.tick().await;
        while let Some(result) = jobs.try_join_next() {
            if !matches!(result, Ok(true)) {
                monitor.evidence.store(false, Ordering::Relaxed);
            }
        }
        let at = now();
        monitor.heartbeat.store(at, Ordering::Relaxed);
        if !configured {
            let ledger = monitor.ledger.clone();
            let jobs = monitor.config.jobs();
            configured = tokio::task::spawn_blocking(move || ledger.configure(&jobs))
                .await
                .is_ok_and(|result| result.is_ok());
            if !configured {
                continue;
            }
        }
        let ledger = monitor.ledger.clone();
        let config = monitor.config.clone();
        let protocol_for_tick = protocol.clone();
        let should_prune = at >= prune_at;
        let tick = tokio::task::spawn_blocking(move || {
            dispatch(at, should_prune, &config, &ledger, &protocol_for_tick)
        })
        .await;
        let Ok(Ok((intents, recovery, cleaned))) = tick else {
            monitor.evidence.store(false, Ordering::Relaxed);
            continue;
        };
        if should_prune {
            prune_at = at + 30;
            if !cleaned {
                monitor.cleanup_failure.store(at, Ordering::Relaxed);
                tracing::warn!("journey evidence pruning failed");
            }
        }
        monitor.evidence.store(cleaned, Ordering::Relaxed);
        for intent in intents {
            let protocol = protocol.clone();
            let ledger = monitor.ledger.clone();
            jobs.spawn(async move {
                let result = first(&protocol, &intent).await;
                trace(&intent, &result);
                tokio::task::spawn_blocking(move || ledger.first(&intent, &result))
                    .await
                    .is_ok_and(|result| result.is_ok())
            });
        }
        for recovery in recovery {
            let protocol = protocol.clone();
            let ledger = monitor.ledger.clone();
            jobs.spawn(async move {
                let result = recover(&protocol, &recovery).await;
                trace(&recovery.intent, &result);
                tokio::task::spawn_blocking(move || {
                    ledger.recovered(&recovery.intent, recovery.attempt, &result)
                })
                .await
                .is_ok_and(|result| result.is_ok())
            });
        }
    }
}

type Dispatch = (Vec<Intent>, Vec<Recovery>, bool);

fn dispatch(
    at: i64,
    prune: bool,
    config: &Config,
    ledger: &Ledger,
    protocol: &Protocol,
) -> Result<Dispatch, &'static str> {
    let recovered = ledger.recovery(at).map_err(|_| "cannot reserve recovery")?;
    let mut intents = Vec::new();
    let mut available = !prune || ledger.prune(at).is_ok();
    for (job, enabled) in config.jobs() {
        if !enabled {
            continue;
        }
        let intent = protocol.intent(job, job.slot_at(at))?;
        match ledger.claim(&intent, at, config.funded.as_ref().map(|v| v.budget)) {
            Ok(Claim::Send) => intents.push(intent),
            Ok(Claim::Skipped(reason)) => tracing::debug!(
                check = job.check.label(),
                mode = job.mode.label(),
                reason = reason.label(),
                "scheduled journey skipped"
            ),
            Ok(Claim::Seen) => {}
            Err(_) => available = false,
        }
    }
    Ok((intents, recovered, available))
}

pub async fn first(protocol: &Protocol, intent: &Intent) -> Observation {
    let start = Instant::now();
    let remaining = (intent.slot + intent.job.mode.deadline() - now()).max(0) as u64;
    let end = start + Duration::from_secs(remaining);
    let mut outcome = Outcome::default();
    if remaining == 0 {
        outcome.settled = true;
        return observation(start, Phase::First, Err(Reason::Missed), outcome);
    }
    let verify_reserve = match intent.job.mode {
        Mode::Read => 0,
        Mode::Deterministic => 4,
        Mode::Funded => 10,
    };
    let first_end = end - Duration::from_secs(verify_reserve.min(remaining.saturating_sub(1)));
    let initial = time::timeout_at(first_end, protocol.first(intent, &mut outcome))
        .await
        .unwrap_or(Err(Reason::Deadline));
    outcome.evidence.delivery_elapsed_ms = Some(start.elapsed().as_millis() as u64);
    if intent.job.mode == Mode::Read
        || outcome.settled
        || matches!(
            initial,
            Err(Reason::CredentialMissing | Reason::CredentialDenied | Reason::CredentialExpired)
        )
    {
        outcome.settled = true;
        return observation(start, Phase::First, initial, outcome);
    }
    let verifying = Instant::now();
    let verified = time::timeout_at(end, verify(protocol, intent, &mut outcome))
        .await
        .unwrap_or(Err(Reason::Deadline));
    outcome.evidence.verification_elapsed_ms = Some(verifying.elapsed().as_millis() as u64);
    let result = initial.and(verified);
    observation(start, Phase::Verify, result, outcome)
}

async fn verify(protocol: &Protocol, intent: &Intent, outcome: &mut Outcome) -> Result<(), Reason> {
    for retry in 0..=2 {
        match protocol.verify(intent, outcome).await {
            Err(Reason::ReceiptMissing | Reason::TerminalMissing | Reason::Transport)
                if retry < 2 =>
            {
                let interval = u64::from(intent.key.as_bytes().last().copied().unwrap_or(0)) % 200
                    + 240 * (retry + 1);
                time::sleep(Duration::from_millis(interval)).await;
            }
            result => return result,
        }
    }
    unreachable!("verification has a final attempt")
}

async fn recover(protocol: &Protocol, recovery: &Recovery) -> Observation {
    let start = Instant::now();
    let mut outcome = Outcome {
        settled: false,
        evidence: recovery
            .previous
            .as_ref()
            .map(|v| v.evidence.clone())
            .unwrap_or_default(),
    };
    let result = time::timeout(
        Duration::from_secs(10),
        protocol.verify(&recovery.intent, &mut outcome),
    )
    .await
    .unwrap_or(Err(Reason::Deadline));
    observation(start, Phase::Reconcile, result, outcome)
}

fn trace(intent: &Intent, observation: &Observation) {
    tracing::debug!(
        check = intent.job.check.label(),
        mode = intent.job.mode.label(),
        scheduled_at = intent.slot,
        phase = observation.phase.label(),
        reason = observation.reason.label(),
        duration_ms = observation.elapsed_ms,
        "journey observation recorded"
    );
}

fn observation(
    start: Instant,
    phase: Phase,
    result: Result<(), Reason>,
    outcome: Outcome,
) -> Observation {
    Observation {
        reason: result.err().unwrap_or(Reason::Passed),
        phase,
        completed_at: now(),
        elapsed_ms: start.elapsed().as_millis() as u64,
        settled: outcome.settled,
        evidence: outcome.evidence,
    }
}
