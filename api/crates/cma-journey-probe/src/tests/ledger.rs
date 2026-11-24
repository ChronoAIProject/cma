use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

use tempfile::TempDir;
use url::Url;

use crate::{
    ledger::{Claim, Ledger},
    model::{now, Check, Mode, Observation, Phase, Reason},
    protocol::Protocol,
};

use super::{config, job};

#[test]
fn duplicate_claim_and_rollback_are_fenced() {
    let dir = TempDir::new().unwrap();
    let config = config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    let protocol = Protocol::new(config.clone()).unwrap();
    let ledger = Ledger::new(&config);
    ledger.init().unwrap();
    assert!(ledger.init().is_err());
    ledger.configure(&config.jobs()).unwrap();
    let at = now() + 300;
    let snapshot = dir.path().join("snapshot");
    fs::copy(&config.state_path, &snapshot).unwrap();
    let job = job(&config, Check::Agui, Mode::Deterministic);
    let slot = job.slot_at(at) + job.mode.interval();
    let intent = Arc::new(protocol.intent(job, slot).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let claims: Vec<_> = (0..2)
        .map(|_| {
            let (barrier, intent, ledger) = (barrier.clone(), intent.clone(), ledger.clone());
            thread::spawn(move || {
                barrier.wait();
                matches!(ledger.claim(&intent, slot, None).unwrap(), Claim::Send)
            })
        })
        .collect();
    assert_eq!(
        claims
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap()))
            .sum::<usize>(),
        1
    );
    let restarted = Ledger::new(&config);
    assert!(matches!(
        restarted.claim(&intent, slot, None).unwrap(),
        Claim::Seen
    ));
    let recovery = restarted.recovery(slot + 34).unwrap();
    assert_eq!(recovery.len(), 1);
    assert_eq!(recovery[0].intent.key, intent.key);
    assert!(recovery[0].previous.is_none());
    let result = Observation {
        phase: Phase::Reconcile,
        reason: Reason::Passed,
        completed_at: slot + 35,
        elapsed_ms: 5,
        settled: true,
        evidence: Default::default(),
        reconciliation_runner_sha: None,
    };
    restarted.recovered(&intent, 1, &result).unwrap();
    let metrics = restarted.metrics().unwrap();
    assert!(metrics.counts.contains(&(
        job.key(),
        "provisional".into(),
        "unknown_submission".into(),
        1
    )));
    assert!(metrics
        .counts
        .contains(&(job.key(), "reconciliation".into(), "passed".into(), 1)));
    assert!(metrics
        .jobs
        .iter()
        .find(|row| row.key == job.key())
        .unwrap()
        .times[3]
        .is_none());
    fs::copy(&snapshot, &config.state_path).unwrap();
    assert!(restarted.available().is_err());
    assert!(restarted.claim(&intent, slot, None).is_err());
    assert!(restarted
        .claim(&protocol.intent(job, slot + 300).unwrap(), slot + 300, None)
        .is_err());
}

#[test]
fn funding_is_off_and_budget_survives_a_restart() {
    let dir = TempDir::new().unwrap();
    let config = config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    assert!(!config.funded_enabled);
    let protocol = Protocol::new(config.clone()).unwrap();
    let ledger = Ledger::new(&config);
    ledger.init().unwrap();
    ledger.configure(&config.jobs()).unwrap();
    let budget = config.funded.as_ref().map(|f| f.budget);
    let paid = job(&config, Check::Agui, Mode::Funded);
    let det = job(&config, Check::Agui, Mode::Deterministic);
    let slot = paid.slot_at(now() + 1200) + 900;
    let intent = protocol.intent(paid, slot).unwrap();
    assert_ne!(intent.key, protocol.intent(det, slot).unwrap().key);
    assert!(matches!(
        ledger.claim(&intent, slot, budget).unwrap(),
        Claim::Seen
    ));
    let mut jobs = config.jobs();
    for (job, enabled) in &mut jobs {
        if job.mode == Mode::Funded {
            *enabled = true;
        }
    }
    ledger.configure(&jobs).unwrap();
    assert!(matches!(
        ledger.claim(&intent, slot, budget).unwrap(),
        Claim::Send
    ));
    let restarted = Ledger::new(&config);
    let next_job = job(&config, Check::Responses, Mode::Funded);
    let next_slot = next_job.slot_at(slot + 120);
    assert!(matches!(
        restarted
            .claim(
                &protocol.intent(next_job, next_slot).unwrap(),
                next_slot,
                budget
            )
            .unwrap(),
        Claim::Skipped(Reason::Budget)
    ));
    assert!(matches!(
        restarted
            .claim(&protocol.intent(det, slot).unwrap(), slot, budget)
            .unwrap(),
        Claim::Send
    ));
}

#[test]
fn missed_slots_and_timestamp_are_durable() {
    let dir = TempDir::new().unwrap();
    let config = config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    let protocol = Protocol::new(config.clone()).unwrap();
    let ledger = Ledger::new(&config);
    ledger.init().unwrap();
    ledger.configure(&config.jobs()).unwrap();
    let job = job(&config, Check::Agent, Mode::Read);
    let slot = job.slot_at(now() + 210);
    let intent = protocol.intent(job, slot).unwrap();
    assert!(matches!(
        ledger.claim(&intent, slot + 25, None).unwrap(),
        Claim::Skipped(Reason::Missed)
    ));
    for _ in 0..2 {
        let metrics = ledger.metrics().unwrap();
        let row = metrics.jobs.iter().find(|r| r.key == job.key()).unwrap();
        assert_eq!(row.times[0], Some(slot));
        assert_eq!(row.times[2], Some(slot + 25));
        assert!(metrics
            .counts
            .iter()
            .any(|(key, _, reason, amount)| key == &job.key()
                && reason == "missed"
                && *amount >= 6));
    }
}

#[test]
fn export_clips_missed_ranges_preserves_provenance_and_skips_disabled_slots() {
    let dir = TempDir::new().unwrap();
    let config = config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    let protocol = Protocol::new(config.clone()).unwrap();
    let ledger = Ledger::new(&config);
    ledger.init().unwrap();
    let read = job(&config, Check::Agent, Mode::Read);
    let start = read.slot_at(now());
    let begin = protocol.intent(read, start).unwrap();
    ledger.configure_at(&[(read, true)], start).unwrap();
    assert!(matches!(
        ledger.claim(&begin, start + 10, None).unwrap(),
        Claim::Skipped(Reason::Missed)
    ));
    let second = protocol.intent(read, start + 90).unwrap();
    assert!(matches!(
        ledger.claim(&second, start + 100, None).unwrap(),
        Claim::Skipped(Reason::Missed)
    ));
    ledger.configure_at(&[(read, false)], start + 100).unwrap();
    ledger.configure_at(&[(read, true)], start + 240).unwrap();
    let third = protocol.intent(read, start + 330).unwrap();
    assert!(matches!(
        ledger.claim(&third, start + 340, None).unwrap(),
        Claim::Skipped(Reason::Missed)
    ));

    let mut upgraded = super::config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    let export_config = Arc::get_mut(&mut upgraded).unwrap();
    export_config.runner_sha = "fedcba9876543210fedcba9876543210fedcba98".into();
    export_config.revision = "rev-2".into();
    let mut output = Vec::new();
    ledger
        .export_to(start + 45, start + 331, &upgraded, &mut output)
        .unwrap();
    let output: Vec<serde_json::Value> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ranges: Vec<_> = output
        .iter()
        .filter(|row| row["kind"] == "missed_range")
        .collect();
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0]["first_scheduled_at"], start + 60);
    assert_eq!(ranges[0]["count"], 1);
    assert_eq!(ranges[1]["first_scheduled_at"], start + 240);
    assert_eq!(ranges[1]["last_scheduled_at"], start + 300);
    assert_eq!(ranges[1]["count"], 3);
    let slot = output
        .iter()
        .find(|row| row["scheduled_at"] == start + 330)
        .unwrap();
    assert_eq!(slot["intent"]["runner_sha"], config.runner_sha);
    assert_eq!(slot["intent"]["configuration"], config.revision);
    assert_eq!(output.last().unwrap()["missed"], 6);
    assert_eq!(output.last().unwrap()["slots"], 6);
}

#[test]
fn disablement_records_missed_slots_from_the_outgoing_enabled_interval() {
    let dir = TempDir::new().unwrap();
    let config = config(&Url::parse("http://127.0.0.1/").unwrap(), dir.path());
    let protocol = Protocol::new(config.clone()).unwrap();
    let ledger = Ledger::new(&config);
    ledger.init().unwrap();
    let check = job(&config, Check::Agui, Mode::Deterministic);
    let slot = check.slot_at(now());
    ledger.configure_at(&[(check, true)], slot).unwrap();
    assert!(matches!(
        ledger
            .claim(&protocol.intent(check, slot).unwrap(), slot + 30, None)
            .unwrap(),
        Claim::Skipped(Reason::Missed)
    ));
    let restarted = Ledger::new(&config);
    restarted
        .configure_at(&[(check, false)], slot + 950)
        .unwrap();
    restarted
        .configure_at(&[(check, false)], slot + 990)
        .unwrap();
    restarted
        .configure_at(&[(check, true)], slot + 1500)
        .unwrap();
    let next = protocol.intent(check, slot + 1500).unwrap();
    assert!(matches!(
        restarted.claim(&next, slot + 1530, None).unwrap(),
        Claim::Skipped(Reason::Missed)
    ));
    let mut output = Vec::new();
    restarted
        .export_to(slot + 1, slot + 1500, &config, &mut output)
        .unwrap();
    let output: Vec<serde_json::Value> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ranges: Vec<_> = output
        .iter()
        .filter(|row| row["kind"] == "missed_range")
        .collect();
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0]["first_scheduled_at"], slot + 300);
    assert_eq!(ranges[0]["last_scheduled_at"], slot + 900);
    assert_eq!(output.last().unwrap()["slots"], 3);
    assert_eq!(output.last().unwrap()["missed"], 3);
}
