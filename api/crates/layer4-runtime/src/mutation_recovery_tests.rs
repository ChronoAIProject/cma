//! Reproduce legacy homes already beyond admission limits, without production repair.
use super::*;
use crate::supervisor::Supervisor;
use crate::writer_fence::SupervisorBoundary;

#[tokio::test]
async fn over_limit_home_starts_only_with_recovery_and_preserves_terminal_bytes() {
    let mut config = config("recovery-startup");
    fill(&config, 1025);
    let path = io::journal_path(&config.state_root, &remove(0).operation_id);
    let original = std::fs::read(&path).unwrap();
    let failure = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .err()
        .unwrap();
    held(failure);
    config.legacy_journal_recovery_enabled = true;
    let supervisor = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    assert!(started(&config).unwrap().is_empty());
    assert_eq!(inventory(&config).unwrap().len(), 1025);
    authority::admit(&config, u64::MAX, &remove(0)).unwrap();
    assert!(matches!(
        begin::<WorktreeRemoveReceipt>(&config, &remove(0)).unwrap(),
        Begin::Completed(_)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    held(authority::admit(&config, 1, &add(5000)).unwrap_err());
    held(authority::admit(&config, 1, &remove(5000)).unwrap_err());
    drop(supervisor);
    let restarted = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    assert_eq!(
        authority::recovery_fence(&config, &remove(0)).unwrap(),
        Some(u64::MAX)
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
    drop(restarted);
    cleanup(&config);
}

fn seed_authority(config: &RuntimeConfig, fence: u64, operation: &Operation) {
    let row = authority::Record::new(config, fence, operation);
    let path = row.inventory_name(config);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(&row).unwrap()).unwrap();
}

#[tokio::test]
async fn recovery_settles_started_home_cleanup_and_never_revives_superseded_create() {
    let mut config = config("recovery-pending");
    config.legacy_journal_recovery_enabled = true;
    fill(&config, 1025);
    let create = Operation {
        operation_id: "agent:old-home:home:create:v1".into(),
        request_sha256: "a".repeat(64),
        payload_sha256: "b".repeat(64),
        subject: Subject::CreateAgentHome {
            agent_id: "old-home".into(),
        },
    };
    let remove = Operation::remove_agent_home(&layer3_adapter::AgentHomeRemoveRequest::new(
        "agent:old-home:home:remove:v1",
        layer3_adapter::AgentSlot::new("old-home"),
    ));
    write_record(&config, &record(&config, &create));
    write_record(&config, &record(&config, &remove));
    seed_authority(&config, 1, &create);
    seed_authority(&config, 2, &remove);
    let supervisor = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    assert!(started(&config).unwrap().is_empty());
    assert!(!config.agent_home("old-home").exists());
    assert!(
        matches!(begin::<serde_json::Value>(&config, &create).unwrap(), Begin::Failed(e) if e.code == "mutation_authority_superseded")
    );
    assert!(matches!(
        begin::<layer3_adapter::wire::AgentHomeRemoveReceipt>(&config, &remove).unwrap(),
        Begin::Completed(_)
    ));
    assert_eq!(
        authority::admit(&config, u64::MAX, &create)
            .unwrap_err()
            .code,
        "mutation_authority_retired"
    );
    drop(supervisor);
    cleanup(&config);
}

#[test]
fn recovery_preserves_failed_unknown_and_mismatched_replays_while_allowing_reserved_cleanup() {
    let mut config = config("recovery-terminal");
    config.legacy_journal_recovery_enabled = true;
    fill(&config, 1025);
    for (index, status) in [(5000, Status::Failed), (5001, Status::Indeterminate)] {
        let operation = add(index);
        let mut row = record(&config, &operation);
        row.status = status;
        row.failure = Some(stored_failure(&RuntimeError::rejected(
            if status == Status::Failed {
                "test_failed"
            } else {
                "runtime_mutation_outcome_unknown"
            },
            "original durable failure",
        )));
        let path = write_record(&config, &row);
        let bytes = std::fs::read(&path).unwrap();
        for fence in [1, 2, u64::MAX] {
            authority::admit(&config, fence, &operation).unwrap();
        }
        let replay = begin::<serde_json::Value>(&config, &operation).unwrap();
        match replay {
            Begin::Failed(error) if status == Status::Failed => {
                assert_eq!(error.detail, "original durable failure")
            }
            Begin::Indeterminate(error) if status == Status::Indeterminate => {
                assert_eq!(error.detail, "original durable failure")
            }
            other => panic!("changed terminal result: {other:?}"),
        }
        let changed = Operation::add_worktree(&WorktreeRequest::new(
            &operation.operation_id,
            "project",
            format!("a-{index}"),
            "b".repeat(40),
        ));
        assert_eq!(
            begin::<serde_json::Value>(&config, &changed)
                .unwrap_err()
                .code,
            "runtime_mutation_replay_mismatch"
        );
        authority::admit(&config, u64::MAX, &remove(index)).unwrap();
        let Begin::New(ticket) = begin::<WorktreeRemoveReceipt>(&config, &remove(index)).unwrap()
        else {
            panic!()
        };
        ticket
            .complete(&terminal(&config, &remove(index)).receipt.unwrap())
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
        assert_eq!(
            authority::admit(&config, u64::MAX, &operation)
                .unwrap_err()
                .code,
            "mutation_authority_retired"
        );
    }
    cleanup(&config);
}

#[test]
fn recovery_validates_all_replacements_before_applying_and_recovers_an_interrupted_commit() {
    let mut config = config("recovery-replacement");
    config.legacy_journal_recovery_enabled = true;
    fill(&config, 1025);
    let operation = remove(5000);
    let path = write_record(&config, &record(&config, &operation));
    let temporary = path.with_file_name(".mutation-journal-terminal.tmp");
    let partial = path.with_file_name(".mutation-journal-partial.tmp");
    let terminal_bytes = encode_record(&terminal(&config, &operation)).unwrap();
    std::fs::write(&temporary, &terminal_bytes).unwrap();
    std::fs::write(&partial, b"{").unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        started(&config).unwrap_err().code,
        "mutation_journal_corrupt"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(temporary.exists() && partial.exists());
    std::fs::remove_file(partial).unwrap(); // fixture restores the modeled interrupted commit
    assert!(started(&config).unwrap().is_empty());
    assert!(!temporary.exists());
    assert_eq!(std::fs::read(&path).unwrap(), terminal_bytes);
    assert!(started(&config).unwrap().is_empty());
    config.legacy_journal_recovery_enabled = false;
    held(started(&config).unwrap_err()); // rollback holds rather than deleting receipts
    assert_eq!(std::fs::read(path).unwrap(), terminal_bytes);
    cleanup(&config);
}

#[test]
fn recovery_envelope_still_reserves_a_terminal_slot_and_limits_physical_entries() {
    let mut config = config("recovery-hard-bound");
    config.legacy_journal_recovery_enabled = true;
    fill(&config, crate::mutation_recovery::JOURNALS - 1);
    assert!(started(&config).unwrap().is_empty());
    write_record(&config, &terminal(&config, &remove(9000)));
    held(started(&config).unwrap_err());
    write_record(&config, &terminal(&config, &remove(9001)));
    held(inventory(&config).unwrap_err());
    cleanup(&config);
}

#[test]
fn recovery_keeps_retained_home_and_active_generation_scopes_separate() {
    let mut config = config("recovery-generation");
    config.legacy_journal_recovery_enabled = true;
    config.home_identity = Some("a".repeat(43));
    std::fs::create_dir_all(&config.home).unwrap();
    crate::persistent_home::initialize(&config).unwrap();
    fill(&config, 1025);
    let close = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:process:close:v1",
        layer3_adapter::AgentSlot::new("process"),
    ));
    let path = write_record(&config, &record(&config, &close));
    seed_authority(&config, u64::MAX, &close);
    assert_eq!(started(&config).unwrap().len(), 1);
    config.generation = 2;
    crate::persistent_home::initialize(&config).unwrap();
    assert!(started(&config).unwrap().is_empty());
    assert!(path.exists());
    assert_eq!(inventory(&config).unwrap().len(), 1025);
    assert_eq!(authority::recovery_fence(&config, &close).unwrap(), None);
    let foreign = RuntimeConfig {
        sandbox_id: "foreign".into(),
        ..config.clone()
    };
    assert!(started(&foreign).is_err());
    cleanup(&config);
}

#[test]
fn recovery_scan_bounds_total_input_bytes_without_dropping_any_receipt() {
    let mut config = config("recovery-byte-bound");
    config.legacy_journal_recovery_enabled = true;
    for index in 0..64 {
        let row = terminal(&config, &remove(index));
        let path = write_record(&config, &row);
        let mut bytes = encode_record(&row).unwrap();
        bytes.resize(MAX_JOURNAL_BYTES as usize, b' '); // Valid JSON whitespace, same receipt.
        std::fs::write(path, bytes).unwrap();
    }
    assert_eq!(inventory(&config).unwrap().len(), 64);
    write_record(&config, &terminal(&config, &remove(64)));
    held(inventory(&config).unwrap_err());
    assert_eq!(
        std::fs::read_dir(config.state_root.join("runtime-mutations"))
            .unwrap()
            .count(),
        65
    );
    cleanup(&config);
}
