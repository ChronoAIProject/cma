use super::*;
use crate::mutation_journal::{Record, Status};
use std::collections::BTreeMap;

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, rows: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let bytes = if metadata.is_dir() {
                b"directory".to_vec()
            } else if metadata.is_file() {
                std::fs::read(&path).unwrap()
            } else {
                b"special".to_vec()
            };
            rows.insert(path.strip_prefix(root).unwrap().into(), bytes);
            if metadata.is_dir() {
                visit(root, &path, rows);
            }
        }
    }
    let mut rows = BTreeMap::new();
    visit(root, root, &mut rows);
    rows
}

fn binding_bytes(fixture: &Fixture) -> u64 {
    std::fs::metadata(fixture.config.state_root.join("home-binding/binding.json"))
        .unwrap()
        .len()
}

fn pipeline(fixture: &Fixture, budget: &mut Budget) -> Result<serde_json::Value> {
    let first = inventory::scan_with_budget(&fixture.config, budget)?;
    let analysis = validation::analyze_with_budget(&fixture.config, &first, budget)?;
    let last = inventory::scan_with_budget(&fixture.config, budget)?;
    assert_eq!(first.summary(), last.summary());
    assert_eq!(first.digest, last.digest);
    Ok(analysis.summary(&first))
}

#[test]
fn exact_aggregate_byte_bound_preserves_scan_summaries_and_source() {
    let fixture = semantic::fixture();
    let bytes = binding_bytes(&fixture);
    let before = snapshot(&fixture.root);
    let mut budget = Budget::new(Limits {
        bytes: bytes * 3,
        ..Limits::default()
    });
    let report = pipeline(&fixture, &mut budget).unwrap();
    assert_eq!(budget.bytes, bytes * 3);
    assert_eq!(report["input_bytes"], bytes);
    assert_eq!(report["semantic_validation"], "mutation_histories_valid");
    assert_eq!(report["admission"], "held");
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn aggregate_byte_refusal_cannot_be_bypassed_by_resetting_a_phase() {
    let fixture = semantic::fixture();
    let bytes = binding_bytes(&fixture);
    let limits = Limits {
        bytes: bytes * 3 - 1,
        ..Limits::default()
    };
    let before = snapshot(&fixture.root);
    // Each real phase fits independently; the same physical bytes must still
    // spend the allowance each time the combined operation reads them.
    let source = inventory::scan(&fixture.config, limits).unwrap();
    validation::analyze_budget(&fixture.config, &source, Budget::new(limits)).unwrap();
    inventory::scan(&fixture.config, limits).unwrap();
    let mut budget = Budget::new(limits);
    let error = pipeline(&fixture, &mut budget).unwrap_err();
    assert_eq!(error.code, "mutation_ledger_migration_capacity");
    assert_eq!(error.detail, format!(
        "runtime mutation ledger migration budget is exhausted; requested_read_bytes_at_least={}", bytes * 3));
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn precharged_budget_keeps_each_inventory_summary_local() {
    let fixture = semantic::fixture();
    let original = fixture.scan().unwrap().summary();
    let mut budget = Budget::new(Limits::default());
    budget.entry(11).unwrap();
    budget.file(17).unwrap();
    let before = (budget.entries, budget.bytes, budget.metadata_bytes);
    let first = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    let after = (budget.entries, budget.bytes, budget.metadata_bytes);
    let second = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    assert_eq!(first.summary(), original);
    assert_eq!(second.summary(), original);
    assert_eq!(first.digest, second.digest);
    assert_eq!(budget.entries - after.0, after.0 - before.0);
    assert_eq!(budget.bytes - after.1, after.1 - before.1);
    assert_eq!(budget.metadata_bytes - after.2, after.2 - before.2);
}

#[test]
fn repeated_enumerations_share_entry_and_metadata_limits() {
    let fixture = semantic::fixture();
    let mut measured = Budget::new(Limits::default());
    inventory::scan_with_budget(&fixture.config, &mut measured).unwrap();
    let before = snapshot(&fixture.root);
    for metadata in [false, true] {
        let mut limits = Limits::default();
        if metadata {
            limits.metadata_bytes = measured.metadata_bytes * 2;
        } else {
            limits.entries = measured.entries * 2;
        }
        let mut budget = Budget::new(limits);
        let first = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
        let second = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
        assert_eq!(first.summary(), second.summary());
        if metadata {
            limits.metadata_bytes -= 1;
        } else {
            limits.entries -= 1;
        }
        let mut budget = Budget::new(limits);
        inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
        let error = inventory::scan_with_budget(&fixture.config, &mut budget)
            .err()
            .unwrap();
        assert_eq!(error.code, "mutation_ledger_migration_capacity");
        assert!(error.detail.contains(if metadata {
            "metadata_bytes_at_least="
        } else {
            "examined_entries_at_least="
        }));
    }
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn replacement_validation_charges_rereads_and_preserves_selection() {
    let fixture = semantic::fixture();
    let started = Record::started(&fixture.config, &semantic::add()).unwrap();
    let canonical = semantic::journal(&fixture.config, &started);
    let failed = started
        .failed(&RuntimeError::rejected("failure", "preserved"), false)
        .unwrap();
    let replacement = canonical.parent().unwrap().join(".mutation-journal-1.tmp");
    std::fs::write(&replacement, failed.encode().unwrap()).unwrap();
    fixture.write("mutation-authority/.authority-1.tmp", b"unpublished");
    let before = snapshot(&fixture.root);
    let mut budget = Budget::new(Limits::default());
    let inventory = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    let physical_bytes: u64 = inventory
        .entries
        .iter()
        .map(|entry| entry.identity.bytes)
        .sum();
    assert_eq!(budget.bytes, physical_bytes);
    let analysis =
        validation::analyze_with_budget(&fixture.config, &inventory, &mut budget).unwrap();
    assert!(
        budget.bytes > physical_bytes * 2,
        "validator must charge its rereads"
    );
    let group = analysis.journals.values().next().unwrap();
    assert_eq!(analysis.journals.len(), 1);
    let selected = group.selected.unwrap();
    assert_eq!(group.replacement, Some(selected));
    assert_eq!(inventory.entries[selected].name, ".mutation-journal-1.tmp");
    assert_eq!(
        inventory.entries[group.canonical.unwrap()].name,
        canonical.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(analysis.summary(&inventory)["failed"], 1);
    let mut limited = Budget::new(Limits {
        bytes: physical_bytes * 2,
        ..Limits::default()
    });
    let inventory = inventory::scan_with_budget(&fixture.config, &mut limited).unwrap();
    let error = validation::analyze_with_budget(&fixture.config, &inventory, &mut limited)
        .err()
        .unwrap();
    assert_eq!(error.code, "mutation_ledger_migration_capacity");
    assert!(error.detail.contains("requested_read_bytes_at_least="));
    assert_eq!(
        Record::decode(&std::fs::read(&canonical).unwrap(), &fixture.config)
            .unwrap()
            .status(),
        Status::Started
    );
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn expiration_between_phases_is_not_a_fresh_deadline() {
    let fixture = semantic::fixture();
    let before = snapshot(&fixture.root);
    let mut budget = Budget::new(Limits::default());
    let inventory = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    budget.start = Instant::now() - Duration::from_secs(301);
    let counters = (budget.entries, budget.bytes, budget.metadata_bytes);
    let error = validation::analyze_with_budget(&fixture.config, &inventory, &mut budget)
        .err()
        .unwrap();
    assert_eq!(error.code, "mutation_ledger_migration_capacity");
    assert!(error.detail.contains("elapsed_nanoseconds_at_least="));
    let error = inventory::scan_with_budget(&fixture.config, &mut budget)
        .err()
        .unwrap();
    assert_eq!(error.code, "mutation_ledger_migration_capacity");
    assert!(error.detail.contains("elapsed_nanoseconds_at_least="));
    assert_eq!(
        (budget.entries, budget.bytes, budget.metadata_bytes),
        counters
    );
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn existing_wrappers_and_fresh_borrowed_cores_agree() {
    let fixture = semantic::fixture();
    semantic::authority(&fixture.config, 1, &semantic::add());
    semantic::journal(
        &fixture.config,
        &Record::started(&fixture.config, &semantic::add()).unwrap(),
    );
    let before = snapshot(&fixture.root);
    let wrapper = fixture.scan().unwrap();
    let core =
        inventory::scan_with_budget(&fixture.config, &mut Budget::new(Limits::default())).unwrap();
    assert_eq!(wrapper.summary(), core.summary());
    assert_eq!(wrapper.digest, core.digest);
    let expected = validation::analyze(&fixture.config, &wrapper)
        .unwrap()
        .summary(&wrapper);
    assert_eq!(
        expected,
        validation::analyze_budget(&fixture.config, &core, Budget::new(Limits::default()))
            .unwrap()
            .summary(&core)
    );
    assert_eq!(
        expected,
        validation::analyze_with_budget(
            &fixture.config,
            &core,
            &mut Budget::new(Limits::default())
        )
        .unwrap()
        .summary(&core)
    );
    assert_eq!(snapshot(&fixture.root), before);
}

#[test]
fn shared_reading_preserves_strict_record_and_format_refusals() {
    for mode in 0..6 {
        let fixture = semantic::fixture();
        match mode {
            0 => {
                std::fs::remove_file(fixture.config.state_root.join("home-binding/binding.json"))
                    .unwrap();
            }
            1 => {
                fixture.write("runtime-mutations/0000000000000000000000000000000000000000000000000000000000000000.json", b"{");
            }
            2 => {
                fixture.write("mutation-ledger-v2.current.json", b"{}");
            }
            3 => {
                fixture.write("workspace-prepare-recovery", b"unsupported");
            }
            4 => {
                std::fs::remove_dir_all(fixture.config.state_root.join("home-binding")).unwrap();
                std::os::unix::fs::symlink(
                    fixture.config.home.clone(),
                    fixture.config.state_root.join("home-binding"),
                )
                .unwrap();
            }
            _ => {
                let path = fixture.config.state_root.join("home-binding/binding.json");
                std::fs::remove_file(&path).unwrap();
                let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                // SAFETY: the CString names an absent entry in this test's private directory.
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
        }
        let before = snapshot(&fixture.root);
        assert_eq!(
            pipeline(&fixture, &mut Budget::new(Limits::default()))
                .unwrap_err()
                .code,
            "mutation_ledger_migration_invalid",
            "mode {mode}"
        );
        assert_eq!(snapshot(&fixture.root), before);
    }
}

#[test]
fn source_replacement_is_refused_and_final_scan_observes_added_entries() {
    let fixture = semantic::fixture();
    let mut budget = Budget::new(Limits::default());
    let first = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    let binding = fixture.config.state_root.join("home-binding/binding.json");
    let bytes = std::fs::read(&binding).unwrap();
    let replacement = fixture.root.join("replacement");
    std::fs::write(&replacement, bytes).unwrap();
    std::fs::rename(replacement, &binding).unwrap();
    let before = snapshot(&fixture.root);
    let error = validation::analyze_with_budget(&fixture.config, &first, &mut budget)
        .err()
        .unwrap();
    assert_eq!(error.code, "mutation_ledger_migration_changed");
    assert_eq!(snapshot(&fixture.root), before);
    let second = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    fixture.write("mutation-authority/.authority-1.tmp", b"unpublished");
    let before = snapshot(&fixture.root);
    let last = inventory::scan_with_budget(&fixture.config, &mut budget).unwrap();
    assert_ne!(second.digest, last.digest);
    assert_eq!(last.entries.len(), second.entries.len() + 1);
    assert_eq!(snapshot(&fixture.root), before);
}
