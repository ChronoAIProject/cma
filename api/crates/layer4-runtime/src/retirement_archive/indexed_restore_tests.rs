//! Synthetic two-store inspection tests; these do not mint an exclusion permit.
use super::*;
use crate::mutation_ledger::retirement_inventory::DeactivationReceipt;
use crate::retirement_archive::indexed_restore::{
    assess_restore_held, test_inspect_and_close, HeldRestoreAssessment, RestoreInspection,
};

fn files(fixture: &Fixture) -> [Vec<u8>; 4] {
    [
        fixture.root.join("active/ledger.sqlite3"),
        fixture.root.join("active/emergency.reserve"),
        fixture.path.join("ledger.sqlite3"),
        fixture.path.join("emergency.reserve"),
    ]
    .map(|path| std::fs::read(path).unwrap())
}

fn request(
    fixture: &Fixture,
    proof: &GroupProof,
    observation: &[u8],
    transition: &DeactivationReceipt,
) -> RestoreInspection {
    RestoreInspection {
        archive_id: fixture.id.clone(),
        proof: proof.clone(),
        observation: observation.to_vec(),
        transition: transition.clone(),
    }
}

async fn inspect(fixture: &Fixture, request: RestoreInspection) -> Result<HeldRestoreAssessment> {
    assess_restore_held(
        fixture.indexed(false).await,
        fixture.active(false).await,
        request,
    )
    .await
}

#[tokio::test]
async fn restore_inspection_verifies_archive_and_source_without_writing_or_authorizing() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, true, false).await;
    let transition = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    let before = files(&fixture);
    let result = inspect(
        &fixture,
        request(&fixture, &proof, &observation, &transition),
    )
    .await
    .unwrap();
    assert_eq!(result.archive_records, u64::from(proof.records));
    assert!(result.archive_bytes > 0);
    assert!(!result.archive_group_complete);
    assert_eq!(result.source.source_sha256, proof.source_sha256);
    assert_eq!(result.source.after_sha256, transition.after_sha256);
    assert_eq!(
        (
            result.source.operations,
            result.source.authority,
            result.source.cleanup
        ),
        (4, 2, 2)
    );
    assert!(result.source.no_newer_effects_proof_required);
    assert!(result.source.retained_backup_verification_required);
    assert!(!result.source.restoration_authorized);
    assert_eq!(files(&fixture), before);
    let replay = inspect(
        &fixture,
        request(&fixture, &proof, &observation, &transition),
    )
    .await
    .unwrap();
    assert_eq!(replay, result);
    assert_eq!(files(&fixture), before);
    let archive = fixture.indexed(false).await;
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
    let output = format!("{} {result:?}", serde_json::to_string(&result).unwrap());
    for private in [
        fixture.config.sandbox_id.as_str(),
        fixture.id.as_str(),
        fixture.root.to_str().unwrap(),
    ] {
        assert!(!output.contains(private));
    }
    assert_eq!(files(&fixture), before);
}

#[tokio::test]
async fn wrong_archive_observation_receipt_and_bounds_refuse_and_close_both_owners() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    let transition = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    let before = files(&fixture);
    for case in 0..8 {
        let mut input = request(&fixture, &proof, &observation, &transition);
        match case {
            0 => input.archive_id = uuid::Uuid::new_v4().to_string(),
            1 => input.archive_id = uuid::Uuid::nil().to_string(),
            2 => input.observation.push(b' '),
            3 => input.proof.source_sha256 = "0".repeat(64),
            4 => input.proof.records = 16_385,
            5 => input.proof.records = 0,
            6 => input.transition.after_revision += 1,
            _ => input.observation.resize(
                layer3_adapter::runtime_ledger_retirement::MAX_LEDGER_RETIREMENT_SNAPSHOT_BYTES + 1,
                b' ',
            ),
        }
        let error = inspect(&fixture, input).await.unwrap_err();
        assert!(!error.retryable);
        assert_eq!(files(&fixture), before, "case {case}");
        // Immediate reopen verifies close completion even on the refusal path.
        fixture.active(false).await.close().await.unwrap();
        fixture.indexed(false).await.close().await.unwrap();
    }
}

#[tokio::test]
async fn damaged_payload_missing_replay_index_and_oversized_archive_refuse_without_writes() {
    for case in 0..4 {
        let fixture = Fixture::new();
        let (proof, observation) = fixture.prepared(6, false, false).await;
        let transition = fixture
            .apply(proof.clone(), observation.clone())
            .await
            .unwrap();
        let archive = fixture.indexed(false).await;
        archive
            .storage
            .call(move |db| {
                let sql = match case {
                    0 => "UPDATE indexed_records SET payload=CAST('damaged' AS BLOB)",
                    1 => "DELETE FROM retired_operations",
                    2 => "UPDATE retired_groups SET bytes=67108865",
                    _ => "DELETE FROM indexed_records WHERE kind=3",
                };
                db.execute(sql, []).unwrap();
                Ok(())
            })
            .await
            .unwrap();
        archive.close().await.unwrap();
        let before = files(&fixture);
        assert!(
            inspect(
                &fixture,
                request(&fixture, &proof, &observation, &transition)
            )
            .await
            .is_err(),
            "case {case}"
        );
        assert_eq!(files(&fixture), before, "case {case}");
    }
}

#[tokio::test]
async fn later_source_effects_refuse_even_with_an_intact_archive() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    let transition = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    let source = fixture.active(false).await;
    fixture.cycle(&source, "newer-work", 6).await;
    source.close().await.unwrap();
    let before = files(&fixture);
    let error = inspect(
        &fixture,
        request(&fixture, &proof, &observation, &transition),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "mutation_ledger_restore_source_changed");
    assert_eq!(files(&fixture), before);
}

#[tokio::test]
async fn completed_group_is_only_reported_and_query_failures_never_release_reserves() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    let transition = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    let archive = fixture.indexed(false).await;
    archive
        .storage
        .call(|db| {
            // Synthetic completed phase; inspection must not turn it back into a hold
            // or mistake per-group completion for restoration authority.
            db.execute("UPDATE retired_groups SET complete=1", [])
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    archive.close().await.unwrap();
    let before = files(&fixture);
    let result = inspect(
        &fixture,
        request(&fixture, &proof, &observation, &transition),
    )
    .await
    .unwrap();
    assert!(result.archive_group_complete);
    assert!(!result.source.restoration_authorized);
    assert_eq!(files(&fixture), before);
    let archive = fixture.indexed(false).await;
    archive
        .storage
        .call(|db| {
            db.execute_batch("CREATE TEMP TABLE indexed_records (unavailable INTEGER)")
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert!(assess_restore_held(
        archive,
        fixture.active(false).await,
        request(&fixture, &proof, &observation, &transition)
    )
    .await
    .is_err());
    assert_eq!(files(&fixture), before);
    fixture.indexed(false).await.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_wait_keeps_both_inspection_owners_until_close_and_changes_no_bytes() {
    let fixture = Fixture::new();
    let (proof, observation) = fixture.prepared(6, false, false).await;
    let transition = fixture
        .apply(proof.clone(), observation.clone())
        .await
        .unwrap();
    let before = files(&fixture);
    let entered = Arc::new(tokio::sync::Notify::new());
    let finished = Arc::new(tokio::sync::Notify::new());
    struct Finished(Arc<tokio::sync::Notify>);
    impl Drop for Finished {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    let drop_signal = Finished(finished.clone());
    let signal = entered.clone();
    let (release, wait) = std::sync::mpsc::channel();
    let future = test_inspect_and_close(
        fixture.indexed(false).await,
        fixture.active(false).await,
        request(&fixture, &proof, &observation, &transition),
        move || {
            let _retained = &drop_signal;
            signal.notify_one();
            wait.recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|_| invalid())
        },
    );
    let task = tokio::spawn(future);
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(Ledger::open(
        fixture.root.join("active"),
        fixture.config.clone(),
        false,
        32_768
    )
    .await
    .is_err());
    assert!(Archive::open_version(
        fixture.path.clone(),
        fixture.config.clone(),
        fixture.id.clone(),
        false,
        32_768,
        Version::Indexed
    )
    .await
    .is_err());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), finished.notified())
        .await
        .unwrap();
    fixture.active(false).await.close().await.unwrap();
    fixture.indexed(false).await.close().await.unwrap();
    assert_eq!(files(&fixture), before);
}
