use super::*;
use crate::mutation_authority::Record as Authority;
use crate::mutation_journal::{Operation, Record, Status};
use layer3_adapter::{AgentCloseRequest, AgentSlot, WorktreeRef, WorktreeRequest};

pub(super) fn fixture() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(43));
    crate::persistent_home::initialize(&fixture.config).unwrap();
    fixture
}

pub(super) fn add() -> Operation {
    Operation::add_worktree(&WorktreeRequest::new(
        "worktree:workspace:agent:add:v1",
        "workspace",
        "agent",
        "a".repeat(40),
    ))
}
pub(super) fn remove() -> Operation {
    Operation::remove_worktree(&WorktreeRef::new(
        "worktree:workspace:agent:remove:v1",
        "workspace",
        "agent",
    ))
}
fn changed_add() -> Operation {
    Operation::add_worktree(&WorktreeRequest::new(
        "worktree:workspace:agent:add:v1",
        "workspace",
        "agent",
        "b".repeat(40),
    ))
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
pub(super) fn authority(config: &RuntimeConfig, fence: u64, operation: &Operation) -> PathBuf {
    let record = Authority::new(config, fence, operation);
    let path = record.inventory_name(config);
    write(&path, &serde_json::to_vec(&record).unwrap());
    path
}
pub(super) fn journal(config: &RuntimeConfig, record: &Record) -> PathBuf {
    let path = record.inventory_name(config);
    write(&path, &record.encode().unwrap());
    path
}
fn analyze(fixture: &Fixture) -> Result<validation::Analysis> {
    validation::analyze(&fixture.config, &fixture.scan()?)
}

#[test]
fn validates_all_records_before_folding_thousands_of_fences() {
    let fixture = fixture();
    for fence in 1..=4097 {
        authority(&fixture.config, fence, &add());
    }
    authority(&fixture.config, 5000, &remove());
    authority(&fixture.config, u64::MAX, &remove());
    let temporary = fixture.write("mutation-authority/.authority-1.tmp", b"partial");
    let inventory = fixture.scan().unwrap();
    let result = validation::analyze(&fixture.config, &inventory).unwrap();
    assert_eq!(result.authority.len(), 1);
    let survivors = result.authority.values().next().unwrap();
    let fences = survivors
        .iter()
        .map(|index| {
            let entry = &inventory.entries[index.unwrap()];
            let root = &inventory.roots[entry.directory];
            let path = root.path.join(&entry.name);
            Authority::decode_inventory(&std::fs::read(&path).unwrap(), &fixture.config, &path)
                .unwrap()
                .authority_fence
        })
        .collect::<Vec<_>>();
    assert_eq!(fences, [4097, 5000, u64::MAX]);
    assert_eq!(std::fs::read(temporary).unwrap(), b"partial");
    assert_eq!(result.summary(&inventory)["admission"], "held");
    assert_eq!(result.summary(&inventory)["journal_operations"], 0);
}

#[test]
fn conflicting_earlier_identity_and_recreation_cannot_be_hidden_by_high_water() {
    for recreate in [false, true] {
        let fixture = fixture();
        authority(&fixture.config, 1, &add());
        if recreate {
            authority(&fixture.config, 2, &remove());
            authority(&fixture.config, 3, &add());
        } else {
            authority(&fixture.config, 2, &changed_add());
            authority(&fixture.config, u64::MAX, &add());
        }
        assert_eq!(
            analyze(&fixture).err().unwrap().code,
            "mutation_ledger_migration_invalid"
        );
    }
}

#[test]
fn staged_journal_interpretation_preserves_exact_chosen_bytes_and_all_statuses() {
    let fixture = fixture();
    let started = Record::started(&fixture.config, &add()).unwrap();
    let path = journal(&fixture.config, &started);
    let failed = started
        .clone()
        .failed(&RuntimeError::rejected("failure", "failure detail"), false)
        .unwrap();
    let bytes = serde_json::to_vec_pretty(&failed).unwrap();
    let temp = path.parent().unwrap().join(".mutation-journal-2.tmp");
    write(&temp, &bytes);
    write(
        &path.parent().unwrap().join(".mutation-journal-1.tmp"),
        &failed.encode().unwrap(),
    );
    let inventory = fixture.scan().unwrap();
    let result = validation::analyze(&fixture.config, &inventory).unwrap();
    assert_eq!(result.summary(&inventory)["failed"], 1);
    let chosen = result.journals.values().next().unwrap().selected.unwrap();
    assert_eq!(
        inventory
            .read(chosen, &mut Budget::new(Limits::default()))
            .unwrap(),
        bytes
    );
    assert_eq!(
        Record::decode(&std::fs::read(&path).unwrap(), &fixture.config)
            .unwrap()
            .status(),
        Status::Started
    );
    assert!(temp.exists());
}

#[test]
fn refuses_orphan_started_conflicting_and_changed_identity_replacements() {
    for mode in 0..5 {
        let fixture = fixture();
        let started = Record::started(&fixture.config, &add()).unwrap();
        let failed = started
            .clone()
            .failed(&RuntimeError::rejected("failure", "first"), false)
            .unwrap();
        let path = started.inventory_name(&fixture.config);
        if mode != 0 {
            journal(&fixture.config, if mode == 4 { &failed } else { &started });
        }
        let replacement = match mode {
            1 => started.clone(),
            2 => Record::started(&fixture.config, &changed_add())
                .unwrap()
                .failed(&RuntimeError::rejected("failure", "first"), false)
                .unwrap(),
            4 => started
                .failed(&RuntimeError::rejected("failure", "second"), false)
                .unwrap(),
            _ => failed.clone(),
        };
        write(
            &path.parent().unwrap().join(".mutation-journal-1.tmp"),
            &replacement.encode().unwrap(),
        );
        if mode == 3 {
            let other = Record::started(&fixture.config, &add())
                .unwrap()
                .failed(&RuntimeError::rejected("failure", "second"), false)
                .unwrap();
            write(
                &path.parent().unwrap().join(".mutation-journal-2.tmp"),
                &other.encode().unwrap(),
            );
        }
        assert_eq!(
            analyze(&fixture).err().unwrap().code,
            "mutation_ledger_migration_invalid",
            "mode {mode}"
        );
    }
}

#[test]
fn unchanged_terminal_keeps_canonical_bytes_and_started_stays_pending() {
    let fixture = fixture();
    let record = Record::started(&fixture.config, &add()).unwrap();
    let path = journal(&fixture.config, &record);
    let inventory = fixture.scan().unwrap();
    let result = validation::analyze(&fixture.config, &inventory).unwrap();
    assert_eq!(result.summary(&inventory)["started"], 1);
    let terminal = record
        .failed(&RuntimeError::rejected("failure", "same"), false)
        .unwrap();
    let bytes = serde_json::to_vec_pretty(&terminal).unwrap();
    write(&path, &bytes);
    write(
        &path.parent().unwrap().join(".mutation-journal-1.tmp"),
        &terminal.encode().unwrap(),
    );
    let inventory = fixture.scan().unwrap();
    let result = validation::analyze(&fixture.config, &inventory).unwrap();
    let selected = result.journals.values().next().unwrap().selected.unwrap();
    assert!(!inventory.entries[selected].temporary);
    assert_eq!(
        inventory
            .read(selected, &mut Budget::new(Limits::default()))
            .unwrap(),
        bytes
    );
}

#[test]
fn preserves_completed_and_indeterminate_receipts_in_historical_process_scopes() {
    let fixture = fixture();
    let operation = Operation::close_agent(&AgentCloseRequest::new(
        "agent:agent:close:v1",
        AgentSlot::new("agent"),
    ));
    for generation in [2, 7] {
        let mut scoped = fixture.config.clone();
        scoped.generation = generation;
        authority(&scoped, generation, &operation);
        let started = Record::started(&scoped, &operation).unwrap();
        let record = if generation == 2 {
            started
                .completed(&layer3_adapter::wire::AgentCloseReceipt {
                    operation_id: operation.operation_id.clone(),
                    request_sha256: operation.request_sha256.clone(),
                    agent_id: AgentSlot::new("agent"),
                })
                .unwrap()
        } else {
            started
                .failed(
                    &RuntimeError::rejected("agent_close_outcome_unknown", "unknown"),
                    true,
                )
                .unwrap()
        };
        journal(&scoped, &record);
    }
    let inventory = fixture.scan().unwrap();
    let result = validation::analyze(&fixture.config, &inventory).unwrap();
    assert_eq!(result.authority.len(), 2);
    assert_eq!(result.summary(&inventory)["completed"], 1);
    assert_eq!(result.summary(&inventory)["indeterminate"], 1);
    assert_eq!(result.summary(&inventory)["writer_exclusion"], "unproved");
}

#[test]
fn refuses_canonical_corruption_foreign_scope_and_input_drift() {
    let fixture = fixture();
    let path = journal(
        &fixture.config,
        &Record::started(&fixture.config, &add()).unwrap(),
    );
    let inventory = fixture.scan().unwrap();
    std::fs::write(&path, b"{").unwrap();
    assert_eq!(
        validation::analyze(&fixture.config, &inventory)
            .err()
            .unwrap()
            .code,
        "mutation_ledger_migration_changed"
    );
    assert_eq!(
        analyze(&fixture).err().unwrap().code,
        "mutation_ledger_migration_invalid"
    );
    let mut foreign = fixture.config.clone();
    foreign.sandbox_id = "another-sandbox".into();
    journal(&foreign, &Record::started(&foreign, &add()).unwrap());
    assert!(analyze(&fixture).is_err());
    std::fs::remove_file(path).unwrap();
    let path = authority(&fixture.config, 1, &add());
    let wrong = fixture
        .config
        .state_root
        .join("process-generation-2/mutation-authority")
        .join(path.file_name().unwrap());
    std::fs::create_dir_all(wrong.parent().unwrap()).unwrap();
    std::fs::rename(path, wrong).unwrap();
    assert!(analyze(&fixture).is_err());
}

#[test]
fn binding_and_auxiliary_validation_are_required_and_json_topology_is_bounded() {
    let fixture = fixture();
    let binding = fixture.config.state_root.join("home-binding/binding.json");
    let saved = std::fs::read(&binding).unwrap();
    std::fs::remove_file(&binding).unwrap();
    assert!(analyze(&fixture).is_err());
    std::fs::write(&binding, &saved).unwrap();
    let mut foreign = fixture.config.clone();
    foreign.generation += 1;
    assert!(validation::analyze(&foreign, &fixture.scan().unwrap()).is_err());
    let path = fixture.write(&format!("workspace-operations/{}", name(1)), b"{}");
    assert!(analyze(&fixture).is_err());
    std::fs::remove_file(path).unwrap();
    write(
        &fixture
            .config
            .checkpoint_root
            .join("runtime-state/install-v2.json"),
        b"{}",
    );
    assert!(analyze(&fixture).is_err());
    assert!(json::check(br#"{"a":[true,null,1,-1,1.25,"text"]}"#).is_ok());
    assert!(json::check(b"[] []").is_err());
    let wide = serde_json::to_vec(&vec![0; 4096]).unwrap();
    assert_eq!(
        json::check(&wide).unwrap_err().code,
        "mutation_ledger_migration_capacity"
    );
}
