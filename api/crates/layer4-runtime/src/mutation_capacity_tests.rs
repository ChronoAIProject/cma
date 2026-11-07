//! Boundary fixtures write validated V1 files directly to avoid quadratic setup.
use super::*;
use crate::mutation_authority as authority;
use crate::mutation_capacity as capacity;
use layer3_adapter::{WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest};

fn config(name: &str) -> RuntimeConfig {
    let root = std::env::temp_dir().join(format!("cma-capacity-{}-{name}", std::process::id()));
    let config = RuntimeConfig::for_test(root);
    let _ = std::fs::remove_dir_all(&config.state_root);
    let _ = std::fs::remove_dir_all(&config.home);
    config
}

fn cleanup(config: &RuntimeConfig) {
    let _ = std::fs::remove_dir_all(&config.state_root);
    let _ = std::fs::remove_dir_all(&config.home);
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
}

fn remove(index: usize) -> Operation {
    Operation::remove_worktree(&WorktreeRef::new(
        format!("worktree:project:a-{index}:remove:v1"),
        "project",
        format!("a-{index}"),
    ))
}

fn add(index: usize) -> Operation {
    Operation::add_worktree(&WorktreeRequest::new(
        format!("worktree:project:a-{index}:add:v1"),
        "project",
        format!("a-{index}"),
        "a".repeat(40),
    ))
}

fn record(config: &RuntimeConfig, operation: &Operation) -> Record {
    Record {
        version: JOURNAL_VERSION,
        sandbox_id: config.sandbox_id.clone(),
        runtime_generation: crate::mutation_scope::generation(config, &operation.subject),
        operation_id: operation.operation_id.clone(),
        request_sha256: operation.request_sha256.clone(),
        payload_sha256: operation.payload_sha256.clone(),
        subject: operation.subject.clone(),
        status: Status::Started,
        receipt: None,
        failure: None,
    }
}

fn write_record(config: &RuntimeConfig, record: &Record) -> PathBuf {
    validation::validate_record(record, config).unwrap();
    let path = io::journal_path(
        &crate::mutation_scope::root(config, &record.subject),
        &record.operation_id,
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, encode_record(record).unwrap()).unwrap();
    path
}

fn terminal(config: &RuntimeConfig, operation: &Operation) -> Record {
    let Subject::RemoveWorktree { workspace, name } = &operation.subject else {
        panic!()
    };
    let mut row = record(config, operation);
    row.status = Status::Completed;
    row.receipt = Some(
        serde_json::to_value(WorktreeRemoveReceipt {
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            workspace: workspace.clone(),
            name: name.clone(),
        })
        .unwrap(),
    );
    row
}

pub(crate) fn fill(config: &RuntimeConfig, count: usize) {
    for i in 0..count {
        write_record(config, &terminal(config, &remove(i)));
    }
}

fn held(error: RuntimeError) {
    assert_eq!(error.code, "mutation_ledger_capacity");
    assert_eq!(error.detail, capacity::DETAIL);
    assert!(!error.retryable);
}

#[test]
fn old_1024_canonicals_cannot_admit_1025_or_start_recovery() {
    let config = config("legacy-bound");
    fill(&config, MAX_JOURNAL_ENTRIES);
    let before = inventory(&config).unwrap();
    held(authority::admit(&config, 1, &remove(1024)).unwrap_err());
    held(begin::<WorktreeRemoveReceipt>(&config, &remove(1024)).unwrap_err());
    held(started(&config).unwrap_err());
    assert_eq!(inventory(&config).unwrap(), before);
    assert!(!config.state_root.join("mutation-authority").exists());
    cleanup(&config);
}

#[test]
fn full_normal_capacity_keeps_replay_cleanup_and_terminal_temp_headroom() {
    let config = config("full-cleanup");
    fill(&config, 1021);
    // Creation owns the last journal plus its cleanup, with one temp free.
    authority::admit(&config, 1, &add(5000)).unwrap();
    let Begin::New(ticket) = begin::<serde_json::Value>(&config, &add(5000)).unwrap() else {
        panic!()
    };
    ticket
        .fail(&RuntimeError::rejected(
            "test_create_failed",
            "synthetic failure",
        ))
        .unwrap();
    held(authority::admit(&config, 1, &remove(6000)).unwrap_err());
    held(authority::admit(&config, 1, &add(6000)).unwrap_err());
    assert!(matches!(
        begin::<WorktreeRemoveReceipt>(&config, &remove(0)).unwrap(),
        Begin::Completed(_)
    ));
    authority::admit(&config, 2, &remove(5000)).unwrap();
    let Begin::New(ticket) = begin::<WorktreeRemoveReceipt>(&config, &remove(5000)).unwrap() else {
        panic!()
    };
    let replacement = terminal(&config, &remove(5000));
    let temporary = ticket
        .path
        .with_file_name(".mutation-journal-interrupted.tmp");
    std::fs::write(&temporary, encode_record(&replacement).unwrap()).unwrap();
    assert_eq!(
        std::fs::read_dir(ticket.path.parent().unwrap())
            .unwrap()
            .count(),
        1024
    );
    // Inventory validates without changing the crash replacement.
    capacity::check(&config, &[]).unwrap();
    assert!(temporary.exists());
    assert!(started(&config).unwrap().is_empty());
    assert!(!temporary.exists());
    let bytes = std::fs::read(&ticket.path).unwrap();
    authority::admit(&config, 3, &remove(5000)).unwrap();
    assert!(matches!(
        begin::<WorktreeRemoveReceipt>(&config, &remove(5000)).unwrap(),
        Begin::Completed(_)
    ));
    assert_eq!(std::fs::read(&ticket.path).unwrap(), bytes);
    held(authority::admit(&config, 4, &remove(6000)).unwrap_err());
    assert_eq!(inventory(&config).unwrap().len(), 1023);
    cleanup(&config);
}

#[test]
fn whole_preparation_pair_is_reserved_before_first_authority() {
    for (count, accepted) in [(1020, false), (1019, true)] {
        let config = config(&format!("pair-{count}"));
        fill(&config, count);
        let home = Operation {
            operation_id: "agent:paired:home:create:v1".into(),
            request_sha256: "a".repeat(64),
            payload_sha256: "b".repeat(64),
            subject: Subject::CreateAgentHome {
                agent_id: "paired".into(),
            },
        };
        let pair = [add(5000), home];
        let result = authority::admit_many(&config, 1, &pair);
        if accepted {
            result.unwrap();
            assert_eq!(authority::records(&config).unwrap().len(), 2);
            // Both authority-only intents, not just journals, own reservations.
            held(authority::admit(&config, 2, &remove(6000)).unwrap_err());
            for operation in &pair {
                assert!(matches!(
                    begin::<serde_json::Value>(&config, operation).unwrap(),
                    Begin::New(_)
                ));
            }
        } else {
            held(result.unwrap_err());
            assert!(authority::records(&config).unwrap().is_empty());
            assert_eq!(inventory(&config).unwrap().len(), count);
        }
        cleanup(&config);
    }
}

#[test]
fn failed_and_indeterminate_intents_never_release_cleanup_reservations() {
    let config = config("ambiguous");
    fill(&config, 1021);
    let operation = add(5000);
    authority::admit(&config, 1, &operation).unwrap();
    let Begin::New(ticket) = begin::<serde_json::Value>(&config, &operation).unwrap() else {
        panic!()
    };
    ticket.indeterminate("synthetic ambiguous create").unwrap();
    held(authority::admit(&config, 2, &remove(6000)).unwrap_err());
    authority::admit(&config, 2, &remove(5000)).unwrap();
    assert!(matches!(
        begin::<serde_json::Value>(&config, &operation).unwrap(),
        Begin::Indeterminate(_)
    ));
    cleanup(&config);
}

#[test]
fn conflict_in_terminal_temporary_holds_before_authority_or_pruning() {
    let config = config("temp-conflict");
    fill(&config, 1022);
    let operation = remove(5000);
    authority::admit(&config, 1, &operation).unwrap();
    let Begin::New(ticket) = begin::<WorktreeRemoveReceipt>(&config, &operation).unwrap() else {
        panic!()
    };
    let mut conflicting = terminal(&config, &operation);
    conflicting.status = Status::Failed;
    conflicting.receipt = None;
    conflicting.failure = Some(stored_failure(&RuntimeError::rejected(
        "test_failure",
        "failure",
    )));
    let a = ticket.path.with_file_name(".mutation-journal-a.tmp");
    let b = ticket.path.with_file_name(".mutation-journal-b.tmp");
    // Use one replacement at a time at the physical boundary.
    std::fs::write(&a, encode_record(&terminal(&config, &operation)).unwrap()).unwrap();
    std::fs::write(&ticket.path, encode_record(&conflicting).unwrap()).unwrap();
    let error = authority::admit(&config, 2, &operation).unwrap_err();
    assert_eq!(error.code, "mutation_journal_corrupt");
    assert_eq!(
        authority::recovery_fence(&config, &operation).unwrap(),
        Some(1)
    );
    assert!(a.exists());
    assert!(!b.exists());
    cleanup(&config);
}

#[test]
fn capacity_uses_persistent_and_only_active_generation_roots() {
    let mut config = config("generation");
    config.home_identity = Some("a".repeat(43));
    std::fs::create_dir_all(&config.home).unwrap();
    crate::persistent_home::initialize(&config).unwrap();
    fill(&config, 1021);
    let close = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:process:close:v1",
        layer3_adapter::AgentSlot::new("process"),
    ));
    authority::admit(&config, 1, &close).unwrap();
    let Begin::New(ticket) = begin::<serde_json::Value>(&config, &close).unwrap() else {
        panic!()
    };
    ticket
        .fail(&RuntimeError::rejected("test_failure", "failure"))
        .unwrap();
    held(authority::admit(&config, 1, &add(7000)).unwrap_err());
    config.generation += 1;
    crate::persistent_home::initialize(&config).unwrap();
    assert_eq!(inventory(&config).unwrap().len(), 1021);
    authority::admit(&config, 1, &add(7000)).unwrap();
    assert_eq!(authority::recovery_fence(&config, &close).unwrap(), None);
    cleanup(&config);
}

#[path = "mutation_capacity_lifecycle_tests.rs"]
mod lifecycle_tests;

#[test]
fn matching_duplicate_temporaries_recover_before_spending_the_last_slot() {
    let config = config("duplicate-boundary");
    fill(&config, 1021);
    let operation = remove(5000);
    authority::admit(&config, 1, &operation).unwrap();
    let Begin::New(ticket) = begin::<WorktreeRemoveReceipt>(&config, &operation).unwrap() else {
        panic!()
    };
    let bytes = encode_record(&terminal(&config, &operation)).unwrap();
    let a = ticket.path.with_file_name(".mutation-journal-copy-a.tmp");
    let b = ticket.path.with_file_name(".mutation-journal-copy-b.tmp");
    std::fs::write(&a, &bytes).unwrap();
    std::fs::write(&b, &bytes).unwrap();
    held(authority::admit(&config, 2, &add(6000)).unwrap_err());
    assert!(a.exists() && b.exists());
    authority::admit(&config, 2, &remove(6000)).unwrap();
    let Begin::New(next) = begin::<WorktreeRemoveReceipt>(&config, &remove(6000)).unwrap() else {
        panic!()
    };
    assert!(!a.exists() && !b.exists());
    assert_eq!(std::fs::read(&ticket.path).unwrap(), bytes);
    next.complete(&terminal(&config, &remove(6000)).receipt.unwrap())
        .unwrap();
    assert_eq!(inventory(&config).unwrap().len(), 1023);
    cleanup(&config);
}

#[test]
fn partial_replacement_is_ambiguous_and_never_frees_capacity() {
    let config = config("partial-replacement");
    let operation = remove(5000);
    authority::admit(&config, 1, &operation).unwrap();
    let Begin::New(ticket) = begin::<WorktreeRemoveReceipt>(&config, &operation).unwrap() else {
        panic!()
    };
    let temporary = ticket.path.with_file_name(".mutation-journal-partial.tmp");
    std::fs::write(&temporary, b"{").unwrap();
    let error = authority::admit(&config, 2, &operation).unwrap_err();
    assert_eq!(error.code, "mutation_journal_corrupt");
    assert_eq!(std::fs::read(&temporary).unwrap(), b"{");
    assert_eq!(
        authority::recovery_fence(&config, &operation).unwrap(),
        Some(1)
    );
    cleanup(&config);
}

#[path = "mutation_journal_lookup_tests.rs"]
mod lookup_tests;
