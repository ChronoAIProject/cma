use rusqlite::Connection;

use crate::model::{Observation, Phase};

use super::*;

#[tokio::test]
async fn first_observation_survives_publication_retry_after_another_worker_recovers() {
    let dir = TempDir::new().unwrap();
    let (server, task) = serve(Fault::None).await;
    let config = config(&server.base, dir.path());
    let protocol = Protocol::new(config.clone()).unwrap();
    let a = Ledger::new(&config);
    let b = Ledger::new(&config);
    a.init().unwrap();
    let check = job(&config, Check::Agui, Mode::Deterministic);
    let slot = now();
    a.configure_at(&[(check, true)], slot - 1).unwrap();
    let intent = protocol.intent(check, slot).unwrap();
    assert!(matches!(a.claim(&intent, slot, None).unwrap(), Claim::Send));
    let first = first(&protocol, &intent).await;
    assert_eq!(first.reason, Reason::Passed);
    assert!(first.completed_at < slot + check.mode.deadline());

    let db = Connection::open(&config.state_path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_write BEFORE UPDATE OF first_result ON slots
        BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;",
    )
    .unwrap();
    assert!(a.first(&intent, &first).is_err());
    let recovery_at = slot + check.mode.deadline() + 4;
    assert!(matches!(
        b.claim(&intent, recovery_at, None).unwrap(),
        Claim::Seen
    ));
    let work = b.recovery(recovery_at).unwrap();
    assert_eq!(work.len(), 1);
    assert!(work[0].previous.is_none());
    let mut verified = Outcome::default();
    protocol
        .verify(&work[0].intent, &mut verified)
        .await
        .unwrap();
    let recovered = Observation {
        reason: Reason::Passed,
        phase: Phase::Reconcile,
        completed_at: recovery_at,
        elapsed_ms: 10,
        settled: verified.settled,
        evidence: verified.evidence,
        reconciliation_runner_sha: Some(config.runner_sha.clone()),
    };
    b.recovered(&intent, work[0].attempt, &recovered).unwrap();

    db.execute_batch("DROP TRIGGER fail_write").unwrap();
    a.first(&intent, &first).unwrap();
    a.first(&intent, &first).unwrap();
    let mut output = Vec::new();
    a.export_to(slot, slot + 1, &config, &mut output).unwrap();
    let output: Vec<Value> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let row = output
        .iter()
        .find(|row| row["scheduled_at"] == slot)
        .unwrap();
    assert_eq!(row["first"], serde_json::to_value(&first).unwrap());
    assert_eq!(row["recovery_placeholder"]["reason"], "unknown_submission");
    assert_eq!(row["recovery"], json!([recovered]));
    assert!(a.metrics().unwrap().counts.contains(&(
        check.key(),
        "first".into(),
        "passed".into(),
        1
    )));
    assert_eq!(server.admissions.lock().unwrap().len(), 1);
    task.abort();
}
