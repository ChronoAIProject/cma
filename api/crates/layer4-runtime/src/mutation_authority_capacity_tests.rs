use super::*;
use layer3_adapter::{WorktreeRef, WorktreeRequest};

fn config(name: &str) -> RuntimeConfig {
    let config = RuntimeConfig::for_test(std::env::temp_dir().join(format!(
        "cma-authority-capacity-{}-{name}",
        std::process::id()
    )));
    let _ = std::fs::remove_dir_all(&config.state_root);
    config
}

fn add() -> Operation {
    Operation::add_worktree(&WorktreeRequest::new(
        "worktree:project:agent:add:v1",
        "project",
        "agent",
        "a".repeat(40),
    ))
}

fn remove() -> Operation {
    Operation::remove_worktree(&WorktreeRef::new(
        "worktree:project:agent:remove:v1",
        "project",
        "agent",
    ))
}

fn seed(config: &RuntimeConfig, fence: u64, operation: &Operation) -> std::path::PathBuf {
    let record = Record::new(config, fence, operation);
    let path = io::record_path(
        &crate::mutation_scope::root(config, &operation.subject),
        &record,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    path
}

#[test]
fn thousands_of_fresh_fences_retain_only_max_present_and_first_max_absent() {
    let config = config("fresh-fences");
    for fence in 1..=4097 {
        admit(&config, fence, &add()).unwrap();
    }
    assert_eq!(records(&config).unwrap().len(), 1);
    assert_eq!(recovery_fence(&config, &add()).unwrap(), Some(4097));
    assert_eq!(
        admit(&config, 4096, &add()).unwrap_err().code,
        "mutation_authority_stale"
    );
    for fence in 4097..=4100 {
        admit(&config, fence, &remove()).unwrap();
    }
    let records = records(&config).unwrap();
    assert_eq!(records.len(), 3);
    assert!(records
        .iter()
        .any(|row| row.disposition == Disposition::Absent && row.authority_fence == 4097));
    assert!(records
        .iter()
        .any(|row| row.disposition == Disposition::Absent && row.authority_fence == 4100));
    assert_eq!(
        admit(&config, 4101, &add()).unwrap_err().code,
        "mutation_authority_retired"
    );
    std::fs::remove_dir_all(&config.state_root).unwrap();
}

#[test]
fn authority_at_4096_is_held_without_rewrite_but_safe_history_can_fold() {
    for count in [4095, 4096] {
        let config = config(&format!("bound-{count}"));
        for fence in 1..=count {
            seed(&config, fence, &add());
        }
        let result = admit(&config, count + 1, &add());
        if count == 4096 {
            let error = result.unwrap_err();
            assert_eq!(error.code, "mutation_ledger_capacity");
            assert!(!error.retryable);
            assert_eq!(records(&config).unwrap().len(), 4096);
            assert_eq!(recovery_fence(&config, &add()).unwrap(), Some(4096));
        } else {
            result.unwrap();
            assert_eq!(records(&config).unwrap().len(), 1);
            assert_eq!(recovery_fence(&config, &add()).unwrap(), Some(4096));
        }
        std::fs::remove_dir_all(&config.state_root).unwrap();
    }
}

#[test]
fn crash_after_publish_before_any_or_some_deletes_keeps_newest_authority() {
    for remaining in [vec![1, 2, 3], vec![2, 3], vec![3]] {
        let config = config(&format!("crash-{}", remaining.len()));
        for fence in remaining {
            seed(&config, fence, &add());
        }
        // Model a durable publish followed by zero/some/all predecessor deletes.
        assert_eq!(recovery_fence(&config, &add()).unwrap(), Some(3));
        assert_eq!(
            admit(&config, 2, &remove()).unwrap_err().code,
            "mutation_authority_stale"
        );
        admit(&config, 3, &add()).unwrap();
        assert_eq!(records(&config).unwrap().len(), 1);
        admit(&config, 3, &remove()).unwrap();
        for fence in [4, 5] {
            seed(&config, fence, &remove());
        }
        admit(&config, 5, &remove()).unwrap();
        let rows = records(&config).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows
            .iter()
            .any(|row| row.disposition == Disposition::Absent && row.authority_fence == 3));
        assert_eq!(
            admit(&config, 6, &add()).unwrap_err().code,
            "mutation_authority_retired"
        );
        std::fs::remove_dir_all(&config.state_root).unwrap();
    }
}

#[test]
fn hidden_identity_conflict_and_illegal_recreation_hold_before_pruning() {
    for recreate in [false, true] {
        let config = config(&format!("invalid-{recreate}"));
        seed(&config, 1, &add());
        if recreate {
            seed(&config, 2, &remove());
            seed(&config, 3, &add());
        } else {
            let mut changed = add();
            let Subject::AddWorktree { base, .. } = &mut changed.subject else {
                panic!()
            };
            *base = "b".repeat(40);
            changed = Operation::add_worktree(&WorktreeRequest::new(
                &changed.operation_id,
                "project",
                "agent",
                "b".repeat(40),
            ));
            seed(&config, 2, &changed);
            seed(&config, 3, &add());
        }
        assert_eq!(
            admit(&config, 4, &remove()).unwrap_err().code,
            "mutation_authority_corrupt"
        );
        assert_eq!(
            std::fs::read_dir(config.state_root.join("mutation-authority"))
                .unwrap()
                .count(),
            3
        );
        std::fs::remove_dir_all(&config.state_root).unwrap();
    }
}

#[test]
fn first_partial_pair_admission_keeps_its_journal_and_cleanup_reservations() {
    let config = config("partial-pair");
    seed(&config, 7, &add()); // interrupted pair before its second authority write
    let rows = records(&config).unwrap();
    let occupancy = crate::mutation_capacity::occupancy(&[], &rows, &[]).unwrap();
    assert_eq!(occupancy.missing_receipts, 1);
    assert_eq!(occupancy.cleanup_receipts, 1);
    assert_eq!(occupancy.reserved_authority_records, 3);
    admit(&config, 8, &add()).unwrap();
    assert_eq!(records(&config).unwrap().len(), 1);
    std::fs::remove_dir_all(&config.state_root).unwrap();
}

#[test]
fn failure_to_sync_a_survivor_never_deletes_a_predecessor() {
    let config = config("sync-failure");
    let predecessor = seed(&config, 1, &add());
    let survivor = seed(&config, 2, &add());
    let snapshot = files(&config).unwrap();
    // Force the retained-file sync to fail after validation, before deletion.
    std::fs::remove_file(&survivor).unwrap();
    let error = fold::prune(snapshot).err().unwrap();
    assert_eq!(error.code, "mutation_authority_unavailable");
    assert!(predecessor.exists());
    std::fs::remove_dir_all(&config.state_root).unwrap();
}
