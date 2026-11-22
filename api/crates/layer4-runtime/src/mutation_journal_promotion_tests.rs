use super::durability::{Entrance, FaultGuard, Fixture, Step};
use super::*;

fn selected_temp_sync_failure(entrance: Entrance) {
    for status in [Status::Completed, Status::Failed, Status::Indeterminate] {
        let fixture = Fixture::new(
            &format!("promotion-file-{status:?}-{entrance:?}"),
            status,
            true,
        );
        {
            let failure = Step::File(fixture.temporary.clone());
            let guard = FaultGuard::arm(Some(failure.clone()));
            let error = entrance
                .run(&fixture)
                .expect_err("promotion must sync the selected temporary before rename");
            assert_eq!(error.code, "mutation_journal_io_failed");
            assert_eq!(guard.attempted(), vec![failure]);
            assert!(guard.completed().is_empty());
            assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.started_bytes);
            assert_eq!(
                std::fs::read(&fixture.temporary).unwrap(),
                fixture.terminal_bytes
            );
        }
        let guard = FaultGuard::arm(None);
        entrance.run(&fixture).unwrap();
        assert_eq!(
            guard.completed(),
            vec![
                Step::File(fixture.temporary.clone()),
                Step::Rename(fixture.temporary.clone(), fixture.path.clone()),
                Step::Directory(fixture.directory().into()),
            ]
        );
        assert!(!fixture.temporary.exists());
        fixture.assert_replay();
    }
}

#[test]
fn selected_temp_sync_failure_preserves_evidence_on_replay() {
    selected_temp_sync_failure(Entrance::Replay);
}

#[test]
fn selected_temp_sync_failure_preserves_evidence_on_recovery() {
    selected_temp_sync_failure(Entrance::Recovery);
}

#[test]
fn failed_promotion_rename_preserves_both_records_for_retry() {
    let fixture = Fixture::new("promotion-rename", Status::Completed, true);
    {
        let failure = Step::Rename(fixture.temporary.clone(), fixture.path.clone());
        let guard = FaultGuard::arm(Some(failure));
        assert_eq!(
            Entrance::Recovery.run(&fixture).unwrap_err().code,
            "mutation_journal_io_failed"
        );
        assert_eq!(
            guard.completed(),
            vec![Step::File(fixture.temporary.clone())]
        );
        assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.started_bytes);
        assert_eq!(
            std::fs::read(&fixture.temporary).unwrap(),
            fixture.terminal_bytes
        );
    }
    fixture.assert_replay();
}

#[test]
fn failed_directory_sync_after_promotion_retains_terminal_and_retry_syncs_it() {
    for status in [Status::Completed, Status::Failed, Status::Indeterminate] {
        for entrance in [Entrance::Replay, Entrance::Recovery] {
            let fixture = Fixture::new(
                &format!("promotion-directory-{status:?}-{entrance:?}"),
                status,
                true,
            );
            {
                let guard = FaultGuard::arm(Some(Step::Directory(fixture.directory().into())));
                assert_eq!(
                    entrance.run(&fixture).unwrap_err().code,
                    "mutation_journal_io_failed"
                );
                assert_eq!(
                    guard.completed(),
                    vec![
                        Step::File(fixture.temporary.clone()),
                        Step::Rename(fixture.temporary.clone(), fixture.path.clone())
                    ]
                );
                assert_eq!(
                    std::fs::read(&fixture.path).unwrap(),
                    fixture.terminal_bytes
                );
                assert!(!fixture.temporary.exists());
            }
            let guard = FaultGuard::arm(None);
            entrance.run(&fixture).unwrap();
            assert_eq!(
                guard.completed(),
                vec![
                    Step::File(fixture.path.clone()),
                    Step::Directory(fixture.directory().into())
                ]
            );
            fixture.assert_replay();
        }
    }
}

#[test]
fn matching_temporaries_are_removed_only_after_terminal_is_durable() {
    for promotion in [false, true] {
        let fixture = Fixture::new(
            &format!("matching-temps-{promotion}"),
            Status::Completed,
            promotion,
        );
        std::fs::write(&fixture.temporary, &fixture.terminal_bytes).unwrap();
        let duplicate = fixture
            .path
            .with_file_name(".mutation-journal-duplicate.tmp");
        std::fs::write(&duplicate, &fixture.terminal_bytes).unwrap();
        {
            let guard = FaultGuard::arm(Some(Step::Directory(fixture.directory().into())));
            assert_eq!(
                Entrance::Recovery.run(&fixture).unwrap_err().code,
                "mutation_journal_io_failed"
            );
            assert!(guard
                .attempted()
                .iter()
                .all(|step| !matches!(step, Step::Remove(_))));
            assert_eq!(std::fs::read(&duplicate).unwrap(), fixture.terminal_bytes);
            assert_eq!(
                std::fs::read(&fixture.path).unwrap(),
                fixture.terminal_bytes
            );
            if !promotion {
                assert!(fixture.temporary.exists());
            }
        }
        let guard = FaultGuard::arm(None);
        fixture.assert_replay();
        assert!(!duplicate.exists());
        assert!(!fixture.temporary.exists());
        let steps = guard.completed();
        assert_eq!(
            &steps[..2],
            &[
                Step::File(fixture.path.clone()),
                Step::Directory(fixture.directory().into())
            ]
        );
        assert!(steps[2..]
            .iter()
            .all(|step| matches!(step, Step::Remove(_))));
    }
}

fn snapshot(directory: &std::path::Path) -> BTreeMap<PathBuf, Vec<u8>> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect()
}

#[test]
fn inventory_observes_terminal_and_matching_temps_without_sync_or_mutation() {
    for promotion in [false, true] {
        let fixture = Fixture::new(
            &format!("inventory-durability-{promotion}"),
            Status::Completed,
            promotion,
        );
        std::fs::write(&fixture.temporary, &fixture.terminal_bytes).unwrap();
        let duplicate = fixture
            .path
            .with_file_name(".mutation-journal-duplicate.tmp");
        std::fs::write(duplicate, &fixture.terminal_bytes).unwrap();
        let before = snapshot(fixture.directory());
        let guard = FaultGuard::arm(None);
        assert_eq!(
            inventory(&fixture.config).unwrap(),
            vec![fixture.operation.clone()]
        );
        assert!(guard.attempted().is_empty());
        assert_eq!(snapshot(fixture.directory()), before);
    }
}

#[test]
fn conflicting_or_mismatched_temporaries_are_refused_without_touching_evidence() {
    for scenario in [
        "outcome",
        "operation",
        "request",
        "payload",
        "generation",
        "sandbox",
        "receipt",
        "started",
        "malformed",
        "oversized",
    ] {
        let fixture = Fixture::new(&format!("invalid-temp-{scenario}"), Status::Completed, true);
        let mut other = fixture.terminal.clone();
        match scenario {
            "outcome" => {
                other.status = Status::Failed;
                other.receipt = None;
                other.failure = Some(stored_failure(&RuntimeError::rejected(
                    "other_outcome",
                    "conflicting result",
                )));
            }
            "operation" => other.operation_id.push_str("-other"),
            "request" => other.request_sha256 = "a".repeat(64),
            "payload" => other.payload_sha256 = "b".repeat(64),
            "generation" => other.runtime_generation = Some(1),
            "sandbox" => other.sandbox_id.push_str("-other"),
            "receipt" => {
                other.receipt.as_mut().unwrap()["branch"] = serde_json::json!("other-branch")
            }
            "started" => {
                other.status = Status::Started;
                other.receipt = None;
            }
            _ => {}
        }
        let bytes = match scenario {
            "malformed" => b"{".to_vec(),
            "oversized" => vec![b' '; MAX_JOURNAL_BYTES as usize + 1],
            _ => serde_json::to_vec(&other).unwrap(),
        };
        let bad = fixture
            .path
            .with_file_name(".mutation-journal-conflict.tmp");
        std::fs::write(&bad, bytes).unwrap();
        let before = snapshot(fixture.directory());
        let guard = FaultGuard::arm(None);
        for entrance in [Entrance::Replay, Entrance::Recovery] {
            assert_eq!(
                entrance.run(&fixture).unwrap_err().code,
                "mutation_journal_corrupt",
                "{scenario}"
            );
        }
        assert_eq!(
            inventory(&fixture.config).unwrap_err().code,
            "mutation_journal_corrupt"
        );
        assert!(
            guard.attempted().is_empty(),
            "invalid evidence must fail before IO"
        );
        assert_eq!(snapshot(fixture.directory()), before);
    }
}

#[test]
fn nonregular_temporary_is_refused_before_reconciliation() {
    let fixture = Fixture::new("nonregular-temp", Status::Completed, true);
    std::fs::remove_file(&fixture.temporary).unwrap();
    std::fs::create_dir(&fixture.temporary).unwrap();
    let guard = FaultGuard::arm(None);
    for entrance in [Entrance::Replay, Entrance::Recovery] {
        assert_eq!(
            entrance.run(&fixture).unwrap_err().code,
            "mutation_journal_corrupt"
        );
    }
    assert!(guard.attempted().is_empty());
    assert!(fixture.temporary.is_dir());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.started_bytes);
}

#[cfg(unix)]
#[test]
fn temporary_symlink_is_refused_without_following_or_deleting_it() {
    let fixture = Fixture::new("symlink-temp", Status::Completed, true);
    let outside = fixture.config.state_root.join("outside-record");
    std::fs::rename(&fixture.temporary, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &fixture.temporary).unwrap();
    let guard = FaultGuard::arm(None);
    for entrance in [Entrance::Replay, Entrance::Recovery] {
        assert_eq!(
            entrance.run(&fixture).unwrap_err().code,
            "mutation_journal_io_failed"
        );
    }
    assert!(guard.attempted().is_empty());
    assert!(std::fs::symlink_metadata(&fixture.temporary)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(outside).unwrap(), fixture.terminal_bytes);
    assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.started_bytes);
}

#[test]
fn replacement_in_another_generation_scope_is_refused_without_mutation() {
    let mut config = config("misplaced-temp-scope");
    config.home_identity = Some("a".repeat(43));
    crate::persistent_home::initialize(&config).unwrap();
    let fixture = Fixture::with_config(config, Status::Completed, true);
    let process_directory = fixture
        .config
        .state_root
        .join(format!("process-generation-{}", fixture.config.generation))
        .join("runtime-mutations");
    std::fs::create_dir_all(&process_directory).unwrap();
    let misplaced = process_directory.join(fixture.temporary.file_name().unwrap());
    std::fs::rename(&fixture.temporary, &misplaced).unwrap();
    let guard = FaultGuard::arm(None);
    for entrance in [Entrance::Replay, Entrance::Recovery] {
        let error = entrance.run(&fixture).unwrap_err();
        assert_eq!(error.code, "mutation_journal_corrupt");
        assert!(error.detail.contains("scope"));
    }
    assert!(guard.attempted().is_empty());
    assert_eq!(std::fs::read(misplaced).unwrap(), fixture.terminal_bytes);
    assert_eq!(std::fs::read(&fixture.path).unwrap(), fixture.started_bytes);
}

#[test]
fn individually_valid_replacement_with_different_admission_is_refused() {
    let fixture = Fixture::new("different-admission-temp", Status::Failed, true);
    let mut changed = request();
    changed.base = "f".repeat(40);
    changed.request_sha256 = changed.computed_sha256();
    let operation = Operation::add_worktree(&changed);
    let mut other = fixture.terminal.clone();
    other.subject = operation.subject;
    other.request_sha256 = operation.request_sha256;
    other.payload_sha256 = operation.payload_sha256;
    validation::validate_record(&other, &fixture.config).unwrap();
    std::fs::write(&fixture.temporary, encode_record(&other).unwrap()).unwrap();
    let before = snapshot(fixture.directory());
    let guard = FaultGuard::arm(None);
    for entrance in [Entrance::Replay, Entrance::Recovery] {
        let error = entrance.run(&fixture).unwrap_err();
        assert_eq!(error.code, "mutation_journal_corrupt");
        assert!(error.detail.contains("does not follow"));
    }
    assert!(guard.attempted().is_empty());
    assert_eq!(snapshot(fixture.directory()), before);
}
