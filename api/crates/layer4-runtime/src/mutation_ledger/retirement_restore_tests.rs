use super::*;
use crate::mutation_ledger::{lifecycle_tests::lifecycle, tests::Fixture};
use crate::retirement_archive::GroupProof;
use layer3_adapter::runtime_ledger_retirement::{
    ledger_retired_agent_key, RuntimeLedgerRetirementSnapshot, LEDGER_RETIREMENT_SNAPSHOT_FORMAT,
};

struct Retained {
    context: Context,
    rows: Vec<SourceRow>,
    proof: GroupProof,
    observation: Vec<u8>,
}

impl Retained {
    fn selection(&self, fixture: &Fixture) -> Selection {
        Selection::new(
            &fixture.config,
            self.proof.clone(),
            self.context.clone(),
            self.rows.clone(),
            &self.observation,
        )
        .unwrap()
    }
}

async fn cycle(fixture: &Fixture, source: &Ledger, name: &str) {
    for (index, (operation, receipt)) in lifecycle(&fixture.config, name).into_iter().enumerate() {
        source
            .admit(index as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        let begun = source.begin(operation).await.unwrap();
        source
            .finish(begun.record.completed(&receipt).unwrap())
            .await
            .unwrap();
    }
}

async fn prepare() -> (Fixture, Ledger, Retained, DeactivationReceipt) {
    prepare_source(false).await
}

async fn prepare_source(noncanonical: bool) -> (Fixture, Ledger, Retained, DeactivationReceipt) {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(43));
    let source = fixture.open(true).await;
    cycle(&fixture, &source, "restore-fixture").await;
    if noncanonical {
        source.test_retirement_storage(|db| {
            // Semantically identical fixture payloads with distinct original bytes.
            db.execute_batch("UPDATE operations SET identity=CAST(CAST(identity AS TEXT)||' ' AS BLOB),
                record=CAST(CAST(record AS TEXT)||char(10) AS BLOB);
                UPDATE authority SET present=CASE WHEN present IS NULL THEN NULL ELSE CAST(CAST(present AS TEXT)||' ' AS BLOB) END,
                first_absent=CAST(CAST(first_absent AS TEXT)||' ' AS BLOB),
                last_absent=CAST(CAST(last_absent AS TEXT)||char(10) AS BLOB)").unwrap();
            Ok(())
        }).await.unwrap();
    }
    let key = ledger_retired_agent_key("restore-fixture");
    let mut cursor = None;
    let mut context = None;
    let mut rows = Vec::new();
    loop {
        let page = source.retirement_inventory(cursor).await.unwrap();
        context.get_or_insert(page.context);
        rows.extend(
            page.rows
                .into_iter()
                .filter(|row| row.group().unwrap().is_none_or(|group| group == key)),
        );
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    let context = context.unwrap();
    let observation = serde_json::to_vec(&RuntimeLedgerRetirementSnapshot {
        format: LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
        sandbox_id: fixture.config.sandbox_id.clone(),
        home_identity: fixture.config.home_identity.clone().unwrap(),
        generation: fixture.config.generation,
        sandbox_revision: 1,
        observed_at: "2026-11-11T00:00:00Z".into(),
        source_sha256: "b".repeat(64),
        agents_observed: 1,
        worktrees_observed: 0,
        deleted_agents: vec![key.clone()],
        removed_worktrees: vec![],
    })
    .unwrap();
    let mut hash = Sha256::new();
    context.hash_into(&mut hash);
    for row in &rows {
        row.hash_into(&mut hash);
    }
    let proof = GroupProof {
        key,
        source_sha256: format!("{:x}", hash.finalize()),
        observation_sha256: format!("{:x}", Sha256::digest(&observation)),
        records: rows.len() as u32,
    };
    let retained = Retained {
        context,
        rows,
        proof,
        observation,
    };
    // Synthetic source-only qualification. This is not a production archive or
    // an exclusion permit, and the assessment must not claim either one.
    let receipt = source
        .deactivate_retirement(retained.selection(&fixture), ())
        .await
        .unwrap();
    (fixture, source, retained, receipt)
}

#[path = "retirement_restore_transaction_tests.rs"]
mod transaction_tests;

fn bytes(fixture: &Fixture) -> (Vec<u8>, Vec<u8>) {
    (
        std::fs::read(fixture.database().join("ledger.sqlite3")).unwrap(),
        std::fs::read(fixture.database().join("emergency.reserve")).unwrap(),
    )
}

#[tokio::test]
async fn exact_source_assessment_after_reopen_is_read_only_and_does_not_grant_restore() {
    let (fixture, source, retained, receipt) = prepare().await;
    source.close().await.unwrap();
    let before = bytes(&fixture);
    let source = fixture.open(false).await;
    let snapshot = source.snapshot().await.unwrap();
    let result = source
        .assess_retirement_restore(retained.selection(&fixture), receipt.clone())
        .await
        .unwrap();
    assert_eq!(result.source_sha256, receipt.source_sha256);
    assert_eq!(result.after_sha256, receipt.after_sha256);
    assert_eq!(
        (result.operations, result.authority, result.cleanup),
        (4, 2, 2)
    );
    assert!(result.no_newer_effects_proof_required);
    assert!(result.retained_backup_verification_required);
    assert!(!result.restoration_authorized);
    assert_eq!(
        source
            .assess_retirement_restore(retained.selection(&fixture), receipt)
            .await
            .unwrap(),
        result
    );
    assert_eq!(source.snapshot().await.unwrap(), snapshot);
    source.close().await.unwrap();
    assert_eq!(bytes(&fixture), before);
    let emitted = format!("{} {result:?}", serde_json::to_string(&result).unwrap());
    for private in [
        fixture.config.sandbox_id.as_str(),
        "restore-fixture",
        fixture.database().to_str().unwrap(),
    ] {
        assert!(!emitted.contains(private));
    }
}

#[tokio::test]
async fn every_changed_transition_field_refuses_without_source_or_reserve_writes() {
    let (fixture, source, retained, receipt) = prepare().await;
    let before = bytes(&fixture);
    for case in 0..10 {
        let mut changed = receipt.clone();
        match case {
            0 => changed.source_sha256 = "0".repeat(64),
            1 => changed.before_sha256 = "0".repeat(64),
            2 => changed.after_sha256 = "0".repeat(64),
            3 => changed.before_revision += 1,
            4 => changed.after_revision += 1,
            5 => changed.operations += 1,
            6 => changed.authority += 1,
            7 => changed.cleanup += 1,
            8 => changed.free_pages_before += 1,
            _ => changed.free_pages_after = u64::MAX,
        }
        let error = source
            .assess_retirement_restore(retained.selection(&fixture), changed)
            .await
            .unwrap_err();
        assert_eq!(error.code, "mutation_ledger_restore_source_changed");
        assert!(!error.retryable);
        assert_eq!(bytes(&fixture), before);
    }
    source.close().await.unwrap();
}

#[tokio::test]
async fn a_later_completed_lifecycle_refuses_even_when_the_original_group_is_absent() {
    let (fixture, source, retained, receipt) = prepare().await;
    cycle(&fixture, &source, "newer-work").await;
    let before = bytes(&fixture);
    let snapshot = source.snapshot().await.unwrap();
    assert_eq!(
        source
            .assess_retirement_restore(retained.selection(&fixture), receipt)
            .await
            .unwrap_err()
            .code,
        "mutation_ledger_restore_source_changed"
    );
    assert_eq!(source.snapshot().await.unwrap(), snapshot);
    assert_eq!(bytes(&fixture), before);
    source.close().await.unwrap();
}

#[tokio::test]
async fn inspection_query_errors_preserve_emergency_capacity() {
    let (fixture, source, retained, receipt) = prepare().await;
    let before = bytes(&fixture);
    source
        .test_retirement_storage(|db| {
            db.execute_batch("CREATE TEMP TABLE operations (unavailable INTEGER)")
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        source
            .assess_retirement_restore(retained.selection(&fixture), receipt)
            .await
            .unwrap_err()
            .code,
        "mutation_ledger_restore_source_changed"
    );
    assert_eq!(bytes(&fixture), before);
    source
        .test_retirement_storage(|db| {
            db.execute_batch("DROP TABLE temp.operations").unwrap();
            Ok(())
        })
        .await
        .unwrap();
    source.close().await.unwrap();
    crate::ledger_storage::reserve::require(&fixture.database()).unwrap();
    assert_eq!(bytes(&fixture), before);
}

#[tokio::test]
async fn matching_counters_do_not_hide_a_partially_restored_group() {
    let (fixture, source, retained, receipt) = prepare().await;
    let row = retained
        .rows
        .iter()
        .find(|row| matches!(row, SourceRow::Operation { .. }))
        .unwrap()
        .clone();
    source.test_retirement_storage(move |db| {
        let SourceRow::Operation { scope, id, identity, status, normal, record } = row else { unreachable!() };
        // Deliberate fixture corruption: a row reappears without an accounting
        // revision. Aggregate equality alone must not yield a positive comparison.
        db.execute("INSERT INTO operations(scope,operation_id,identity,status,normal,record) VALUES(?,?,?,?,?,?)",
            rusqlite::params![scope,id,identity,status,normal,record]).unwrap();
        Ok(())
    }).await.unwrap();
    let before = bytes(&fixture);
    assert_eq!(
        source
            .assess_retirement_restore(retained.selection(&fixture), receipt)
            .await
            .unwrap_err()
            .code,
        "mutation_ledger_restore_source_changed"
    );
    assert_eq!(bytes(&fixture), before);
    source.close().await.unwrap();
}
