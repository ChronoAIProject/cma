use super::*;

#[test]
fn owner_stop_close_proof_leaves_pending_journal_replacement_intact() {
    let config = config("close-proof-read-only");
    let operation = remove(5000);
    let started = record(&config, &operation);
    let path = write_record(&config, &started);
    let original = std::fs::read(&path).unwrap();
    let temporary = path.with_file_name(".mutation-journal-unkeyed.tmp");
    let bytes = encode_record(&terminal(&config, &operation)).unwrap();
    std::fs::write(&temporary, &bytes).unwrap();

    assert_eq!(
        find_proof_record(&config, &operation.operation_id, &operation.subject)
            .unwrap_err()
            .code,
        "agent_process_close_journal_unsettled"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(&temporary).unwrap(), bytes);
    cleanup(&config);
}

#[test]
fn direct_replay_recovers_a_matching_temporary_without_a_prior_started_scan() {
    let config = config("keyed-temporary");
    let operation = remove(5000);
    let path = write_record(&config, &record(&config, &operation));
    let temporary = path.with_file_name(".mutation-journal-unkeyed.tmp");
    let bytes = serde_json::to_vec_pretty(&terminal(&config, &operation)).unwrap();
    std::fs::write(&temporary, &bytes).unwrap();
    assert!(admitted(&config, &operation).unwrap());
    assert!(!temporary.exists());
    assert!(matches!(
        begin::<WorktreeRemoveReceipt>(&config, &operation).unwrap(),
        Begin::Completed(_)
    ));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    cleanup(&config);
}

#[test]
fn direct_terminal_replay_cannot_ignore_a_conflicting_replacement() {
    let config = config("keyed-conflict");
    let operation = remove(5000);
    let path = write_record(&config, &terminal(&config, &operation));
    let mut replacement = record(&config, &operation);
    replacement.status = Status::Failed;
    replacement.failure = Some(stored_failure(&RuntimeError::rejected(
        "test_failure",
        "conflicting outcome",
    )));
    let temporary = path.with_file_name(".mutation-journal-conflict.tmp");
    let bytes = encode_record(&replacement).unwrap();
    std::fs::write(&temporary, &bytes).unwrap();
    assert_eq!(
        begin::<WorktreeRemoveReceipt>(&config, &operation)
            .unwrap_err()
            .code,
        "mutation_journal_corrupt"
    );
    assert_eq!(std::fs::read(temporary).unwrap(), bytes);
    cleanup(&config);
}

#[test]
fn keyed_terminal_lookup_keeps_bytes_and_full_startup_validation() {
    let config = config("keyed-startup");
    let operation = remove(5000);
    let row = terminal(&config, &operation);
    let path = write_record(&config, &row);
    let bytes = serde_json::to_vec_pretty(&row).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    for _ in 0..3 {
        assert!(admitted(&config, &operation).unwrap());
        assert!(matches!(
            begin::<WorktreeRemoveReceipt>(&config, &operation).unwrap(),
            Begin::Completed(_)
        ));
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let other = write_record(&config, &terminal(&config, &remove(6000)));
    std::fs::write(other, b"{").unwrap();
    // Startup and supervisor recovery still validate unrelated entries too.
    assert_eq!(
        started(&config).unwrap_err().code,
        "mutation_journal_corrupt"
    );
    assert_eq!(
        authority::admit(&config, 1, &operation).unwrap_err().code,
        "mutation_journal_corrupt"
    );
    cleanup(&config);
}

#[cfg(unix)]
#[test]
fn keyed_terminal_lookup_never_follows_the_target_symlink() {
    let config = config("keyed-symlink");
    let operation = remove(5000);
    let path = write_record(&config, &terminal(&config, &operation));
    let outside = config.state_root.join("outside.json");
    std::fs::rename(&path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let error = begin::<WorktreeRemoveReceipt>(&config, &operation).unwrap_err();
    assert_eq!(error.code, "mutation_journal_io_failed");
    cleanup(&config);
}
