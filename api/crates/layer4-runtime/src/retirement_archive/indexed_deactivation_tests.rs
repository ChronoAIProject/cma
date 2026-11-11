use super::*;
use crate::mutation_ledger::lifecycle_tests::lifecycle;
use crate::retirement_archive::indexed_deactivation::{deactivate_checked, Boundary};
use layer3_adapter::runtime_ledger_retirement::{
    RuntimeLedgerRetirementSnapshot, LEDGER_RETIREMENT_SNAPSHOT_FORMAT,
};

impl Fixture {
    async fn active(&self, create: bool) -> Ledger {
        Ledger::open(
            self.root.join("active"),
            self.config.clone(),
            create,
            32_768,
        )
        .await
        .unwrap()
    }

    fn observation(&self, key: &str) -> Vec<u8> {
        serde_json::to_vec(&RuntimeLedgerRetirementSnapshot {
            format: LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
            sandbox_id: self.config.sandbox_id.clone(),
            home_identity: self.config.home_identity.clone().unwrap(),
            generation: self.config.generation,
            sandbox_revision: 1,
            observed_at: "2026-11-11T00:00:00Z".into(),
            source_sha256: "b".repeat(64),
            agents_observed: 1,
            worktrees_observed: 0,
            deleted_agents: vec![key.into()],
            removed_worktrees: vec![],
        })
        .unwrap()
    }

    async fn cycle(&self, source: &Ledger, name: &str, steps: usize) {
        for (index, (op, receipt)) in lifecycle(&self.config, name)
            .into_iter()
            .take(steps)
            .enumerate()
        {
            source
                .admit(index as u64 + 1, vec![op.clone()])
                .await
                .unwrap();
            let begun = source.begin(op).await.unwrap();
            source
                .finish(begun.record.completed(&receipt).unwrap())
                .await
                .unwrap();
        }
    }

    async fn prepared(&self, steps: usize, live: bool, omit: bool) -> (GroupProof, Vec<u8>) {
        let source = self.active(true).await;
        self.cycle(&source, "retired", steps).await;
        if live {
            self.cycle(&source, "live", 3).await;
        }
        let key = group_key(&lifecycle(&self.config, "retired")[1].0);
        let mut cursor = None;
        let mut context = None;
        let mut rows = Vec::new();
        loop {
            let page = source.retirement_inventory(cursor).await.unwrap();
            context.get_or_insert(page.context);
            for row in page.rows {
                if row.group().unwrap().is_none_or(|group| group == key) {
                    rows.push(row);
                }
            }
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        if omit {
            let index = rows.iter().position(|row| matches!(row, SourceRow::Operation { id, .. } if id.contains(":home:create:"))).unwrap();
            rows.remove(index);
        }
        source.close().await.unwrap();
        let context = context.unwrap();
        let mut proof = proof(&context, &rows, key);
        let observation = self.observation(&proof.key);
        proof.observation_sha256 = format!("{:x}", Sha256::digest(&observation));
        let archive = self.indexed(true).await;
        archive
            .prepare_indexed(proof.clone(), context, move |index| {
                Ok(rows[index as usize].clone())
            })
            .await
            .unwrap();
        archive.close().await.unwrap();
        (proof, observation)
    }

    async fn apply(
        &self,
        proof: GroupProof,
        observation: Vec<u8>,
    ) -> Result<crate::mutation_ledger::retirement_inventory::DeactivationReceipt> {
        deactivate_held(
            self.indexed(false).await,
            self.active(false).await,
            proof,
            observation,
            (),
        )
        .await
    }
}

async fn fingerprint(source: &Ledger, excluding: &str) -> String {
    let mut digest = Sha256::new();
    let mut cursor = None;
    loop {
        let page = source.retirement_inventory(cursor).await.unwrap();
        for row in page.rows {
            if row.group().unwrap().as_deref() != Some(excluding) {
                row.hash_into(&mut digest);
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    format!("{:x}", digest.finalize())
}

#[tokio::test]
async fn indexed_deactivation_preserves_live_shared_state_and_retries_after_reopen() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, true, false).await;
    let source = fixture.active(false).await;
    let before = source.snapshot().await.unwrap();
    let retained = fingerprint(&source, &proof.key).await;
    source.close().await.unwrap();
    let receipt = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    assert_eq!(
        (receipt.operations, receipt.authority, receipt.cleanup),
        (4, 2, 2)
    );
    assert_eq!(receipt.after_revision, receipt.before_revision + 1);
    let source = fixture.active(false).await;
    let after = source.snapshot().await.unwrap();
    assert_eq!(
        (after.operations, after.authority, after.terminal),
        (
            before.operations - 4,
            before.authority - 2,
            before.terminal - 4
        )
    );
    assert_eq!(
        (after.reserved_bytes, after.pending, after.processes),
        (before.reserved_bytes, before.pending, before.processes)
    );
    assert_eq!(fingerprint(&source, &proof.key).await, retained);
    source.close().await.unwrap();
    let archive = fixture.indexed(false).await;
    assert!(
        !archive
            .verify_indexed(proof.clone())
            .await
            .unwrap()
            .complete
    );
    assert!(archive.require_ready().await.is_err());
    for (operation, _) in lifecycle(&fixture.config, "retired")
        .into_iter()
        .skip(1)
        .take(4)
    {
        assert_eq!(
            archive.authorize(&[operation]).await.unwrap_err().code,
            "mutation_resource_retired"
        );
    }
    archive.close().await.unwrap();
    assert_eq!(fixture.apply(proof, observation).await.unwrap(), receipt);
}

#[tokio::test]
async fn indexed_deactivation_refuses_live_omitted_changed_or_unobserved_sources() {
    for case in 0..7 {
        let fixture = Fixture::new();
        let (proof, mut observation) = fixture
            .prepared(if case == 0 { 3 } else { 6 }, true, case == 1)
            .await;
        if case == 2 {
            let source = fixture.active(false).await;
            source.test_retirement_storage(|db| {
                db.execute("UPDATE operations SET identity=CAST(CAST(identity AS TEXT)||' ' AS BLOB) WHERE operation_id LIKE '%retired%'", []).unwrap();
                Ok(())
            }).await.unwrap();
            source.close().await.unwrap();
        }
        if case == 3 {
            observation.push(b' ');
        }
        if case == 4 {
            let source = fixture.active(false).await;
            fixture.cycle(&source, "new-effect", 1).await;
            source.close().await.unwrap();
        }
        if case >= 5 {
            let archive = fixture.indexed(false).await;
            archive
                .storage
                .call(move |db| {
                    if case == 5 {
                        db.execute("DELETE FROM retired_operations", []).unwrap();
                    } else {
                        db.execute("UPDATE retired_groups SET bytes=67108865", [])
                            .unwrap();
                    }
                    Ok(())
                })
                .await
                .unwrap();
            archive.close().await.unwrap();
        }
        let path = fixture.root.join("active/ledger.sqlite3");
        let before = std::fs::read(&path).unwrap();
        assert!(
            fixture.apply(proof, observation).await.is_err(),
            "case {case}"
        );
        assert_eq!(std::fs::read(path).unwrap(), before, "case {case}");
        let archive = fixture.indexed(false).await;
        assert!(archive.require_ready().await.is_err());
        archive.close().await.unwrap();
    }
}

#[tokio::test]
async fn indexed_deactivation_abort_rolls_back_rows_and_accounting_then_recovers() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, true, false).await;
    let source = fixture.active(false).await;
    let before = source.snapshot().await.unwrap();
    let retained = fingerprint(&source, "").await;
    source.test_retirement_storage(|db| {
        db.execute_batch("CREATE TRIGGER fail_deactivation BEFORE DELETE ON authority BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
        Ok(())
    }).await.unwrap();
    let error = deactivate_held(
        fixture.indexed(false).await,
        source,
        proof.clone(),
        observation.clone(),
        (),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "mutation_ledger_storage_hold");
    // Remove only the test-owned trigger before the strict schema reader opens.
    rusqlite::Connection::open(fixture.root.join("active/ledger.sqlite3"))
        .unwrap()
        .execute_batch("DROP TRIGGER fail_deactivation")
        .unwrap();
    let source = fixture.active(false).await;
    let after = source.snapshot().await.unwrap();
    assert_eq!(
        (
            before.revision,
            before.operations,
            before.authority,
            before.terminal,
            before.reserved_bytes
        ),
        (
            after.revision,
            after.operations,
            after.authority,
            after.terminal,
            after.reserved_bytes
        )
    );
    assert_eq!(after.emergency_remaining_bytes, 0);
    assert_eq!(fingerprint(&source, "").await, retained);
    source.close().await.unwrap();
    // Removing the trigger can change geometry. The retained original context
    // must still match; this fixture's tiny schema fits in the same pages.
    fixture.apply(proof, observation).await.unwrap();
}

#[tokio::test]
async fn indexed_deactivation_lost_reply_is_reconstructed_without_another_revision() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    assert!(deactivate_checked(
        fixture.indexed(false).await,
        fixture.active(false).await,
        proof.clone(),
        observation.clone(),
        (),
        |boundary| {
            if matches!(boundary, Boundary::Committed) {
                Err(storage_error())
            } else {
                Ok(())
            }
        }
    )
    .await
    .is_err());
    let source = fixture.active(false).await;
    let before = source.snapshot().await.unwrap();
    source.close().await.unwrap();
    let receipt = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    assert_eq!(receipt.after_revision, before.revision as u64);
    assert_eq!(fixture.apply(proof, observation).await.unwrap(), receipt);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn indexed_deactivation_cancellation_retains_exclusion_until_both_stores_close() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    let gate = Arc::new(tokio::sync::Mutex::new(()));
    let owned = gate.clone().lock_owned().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let archive = fixture.indexed(false).await;
    let source = fixture.active(false).await;
    let task = tokio::spawn(deactivate_checked(
        archive,
        source,
        proof,
        observation,
        owned,
        move |boundary| {
            if matches!(boundary, Boundary::Verified) {
                signal.notify_one();
                wait.recv_timeout(std::time::Duration::from_secs(10))
                    .map_err(|_| invalid())?;
            }
            Ok(())
        },
    ));
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(gate.try_lock().is_err());
    release.send(()).unwrap();
    let _finished = tokio::time::timeout(std::time::Duration::from_secs(10), gate.lock())
        .await
        .unwrap();
    fixture.active(false).await.close().await.unwrap();
    let archive = fixture.indexed(false).await;
    assert!(archive.require_ready().await.is_err());
    archive.close().await.unwrap();
}

#[path = "indexed_deactivation_lifetime_tests.rs"]
mod lifetime_tests;

#[path = "indexed_deactivation_crash_tests.rs"]
mod crash_tests;
