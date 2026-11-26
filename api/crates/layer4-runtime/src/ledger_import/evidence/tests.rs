use super::*;
use crate::ledger_import::evidence::{self, Boundary};
use crate::mutation_journal::{Operation, Record};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;

// Unit fixtures exercise collection independently of the real mount probe.
// The production command always supplies source::readonly.
fn fixture_mount(_: &File) -> Result<()> {
    Ok(())
}

fn collect(fixture: &Fixture) -> Result<Vec<u8>> {
    evidence::collect(
        &fixture.config,
        &mut Budget::new(Limits::default()),
        16 * 1024 * 1024,
        fixture_mount,
        |_| Ok(()),
    )
}

fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let value = if metadata.is_dir() {
                b"directory".to_vec()
            } else if metadata.is_file() {
                std::fs::read(&path).unwrap()
            } else {
                b"special".to_vec()
            };
            result.insert(path.strip_prefix(root).unwrap().into(), value);
            if metadata.is_dir() {
                visit(root, &path, result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(path, path, &mut result);
    result
}

fn report(fixture: &Fixture) -> Value {
    let before = snapshot(&fixture.root);
    let bytes = collect(fixture).unwrap();
    assert_eq!(snapshot(&fixture.root), before);
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(report["enumeration_complete"], true);
    assert_eq!(report["restore_admission"], "HELD");
    assert_eq!(report["semantic_reconciliation"], "NOT_RUN");
    for field in ["native_command_correlation", "runtime_output_boundary"] {
        assert_eq!(report[field], "UNKNOWN");
    }
    for field in ["native_session_inventory", "full_home_inventory"] {
        assert_eq!(report[field], "NOT_ASSESSED");
    }
    for field in [
        "writer_exclusion",
        "external_immutability",
        "provider_identity",
    ] {
        assert_eq!(report[field], "UNVERIFIED");
    }
    assert_eq!(report["retirement_inventory"], "UNSUPPORTED");
    assert!(!String::from_utf8(bytes)
        .unwrap()
        .contains(fixture.root.to_str().unwrap()));
    report
}

#[test]
fn exports_real_mutation_identity_and_every_authority_fence() {
    let fixture = semantic::fixture();
    let operation = semantic::add();
    for fence in [1, 2, 3] {
        semantic::authority(&fixture.config, fence, &operation);
    }
    semantic::journal(
        &fixture.config,
        &Record::started(&fixture.config, &operation).unwrap(),
    );
    std::fs::write(
        fixture.config.home.join("private-content"),
        b"private-user-canary",
    )
    .unwrap();
    let value = report(&fixture);
    let rows = value["records"].as_array().unwrap();
    assert_eq!(rows.len(), fixture.scan().unwrap().entries.len());
    let facts: Vec<_> = rows.iter().filter(|r| !r["fact"].is_null()).collect();
    assert_eq!(facts.len(), 4);
    let fences: Vec<_> = facts
        .iter()
        .filter_map(|r| {
            r["fact"]["fence"]
                .as_str()
                .map(|value| value.parse::<u64>().unwrap())
        })
        .collect();
    assert_eq!(fences.len(), 3);
    for fence in [1, 2, 3] {
        assert!(fences.contains(&fence));
    }
    for row in facts {
        assert_eq!(
            row["fact"]["operation"]["operation_id"],
            operation.operation_id
        );
        assert_eq!(
            row["fact"]["operation"]["request_sha256"],
            operation.request_sha256
        );
        assert_eq!(
            row["fact"]["operation"]["payload_sha256"],
            operation.payload_sha256
        );
        assert_eq!(
            row["fact"]["operation"]["subject_sha256"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert!(row["fact"]["operation"].get("command_id").is_none());
        assert!(row["fact"]["operation"].get("turn_sequence").is_none());
    }
    let text = value.to_string();
    assert!(!text.contains("private-user-canary"));
    assert!(!text.contains("\"base\""));
}

#[test]
fn pending_and_unpublished_records_are_observations_not_recovery() {
    let fixture = semantic::fixture();
    let started = Record::started(&fixture.config, &semantic::add()).unwrap();
    let path = semantic::journal(&fixture.config, &started);
    let failed = started
        .clone()
        .failed(
            &RuntimeError::rejected("failure", "private-failure-canary"),
            false,
        )
        .unwrap();
    std::fs::write(
        path.parent().unwrap().join(".mutation-journal-1.tmp"),
        failed.encode().unwrap(),
    )
    .unwrap();
    fixture.write(
        "mutation-authority/.authority-1.tmp",
        b"partial-private-authority",
    );
    let value = report(&fixture);
    let rows = value["records"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| r["fact"]["status"] == "started" && r["unpublished"] == false));
    assert!(rows
        .iter()
        .any(|r| r["fact"]["status"] == "failed" && r["unpublished"] == true));
    assert!(rows
        .iter()
        .any(|r| r["kind"] == "Authority" && r["unpublished"] == true && r["fact"].is_null()));
    assert_eq!(
        Record::decode(&std::fs::read(path).unwrap(), &fixture.config)
            .unwrap()
            .status(),
        crate::mutation_journal::Status::Started
    );
    assert!(!value.to_string().contains("private-failure-canary"));
    assert!(!value.to_string().contains("partial-private-authority"));
}

#[test]
fn historical_completed_and_current_unknown_outcomes_keep_distinct_scopes() {
    let fixture = semantic::fixture();
    let operation = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:agent:close:v1",
        layer3_adapter::AgentSlot::new("agent"),
    ));
    for generation in [2, 7] {
        let mut config = fixture.config.clone();
        config.generation = generation;
        let started = Record::started(&config, &operation).unwrap();
        let record = if generation == 2 {
            started
                .completed(&layer3_adapter::wire::AgentCloseReceipt {
                    operation_id: operation.operation_id.clone(),
                    request_sha256: operation.request_sha256.clone(),
                    agent_id: layer3_adapter::AgentSlot::new("agent"),
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
        semantic::journal(&config, &record);
    }
    let value = report(&fixture);
    let rows = value["records"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| r["scope"]["kind"] == "process_generation"
            && r["scope"]["generation"] == "2"
            && r["fact"]["status"] == "completed"));
    assert!(rows
        .iter()
        .any(|r| r["scope"]["kind"] == "process_generation"
            && r["scope"]["generation"] == "7"
            && r["fact"]["status"] == "indeterminate"));
}

#[test]
fn source_references_bind_every_observed_record() {
    use sha2::{Digest, Sha256};
    let fixture = semantic::fixture();
    semantic::journal(
        &fixture.config,
        &Record::started(&fixture.config, &semantic::add()).unwrap(),
    );
    let inventory = fixture.scan().unwrap();
    let value = report(&fixture);
    assert_eq!(value["inventory_sha256"], inventory.digest);
    for (index, row) in value["records"].as_array().unwrap().iter().enumerate() {
        let bytes = inventory
            .read(index, &mut Budget::new(Limits::default()))
            .unwrap();
        assert_eq!(row["source_index"], index);
        assert_eq!(row["source_bytes"], bytes.len());
        assert_eq!(
            row["source_sha256"],
            crate::command::hex(&Sha256::digest(bytes))
        );
    }
}

#[test]
fn v2_retirement_and_unknown_format_artifacts_refuse_before_validation() {
    for artifact in [
        "mutation-ledger-v2.current.json",
        ".mutation-ledger-v2.preparing",
        "mutation-ledger-v9",
        "mutation-retirement-v1",
        "mutation-retirement-v1.current.json",
        ".mutation-retirement-v1.preparing",
        "mutation-retirement-v9",
    ] {
        let fixture = semantic::fixture();
        fixture.write(artifact, b"not-opened");
        let before = snapshot(&fixture.root);
        assert_eq!(
            collect(&fixture).unwrap_err().code,
            "ledger_evidence_format_unsupported",
            "{artifact}"
        );
        assert_eq!(snapshot(&fixture.root), before);
    }
}

#[test]
fn incomplete_enumeration_and_invalid_records_never_produce_a_report() {
    for mode in 0..4 {
        let fixture = semantic::fixture();
        let path = semantic::journal(
            &fixture.config,
            &Record::started(&fixture.config, &semantic::add()).unwrap(),
        );
        match mode {
            0 => {
                std::fs::write(path, b"{").unwrap();
            }
            1 => {
                let mut row: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                row["unknown"] = true.into();
                std::fs::write(path, row.to_string()).unwrap();
            }
            2 => {
                std::fs::remove_file(fixture.config.state_root.join("home-binding/binding.json"))
                    .unwrap();
            }
            _ => {
                fixture.write("runtime-mutations/unrecognized", b"unknown");
            }
        }
        let before = snapshot(&fixture.root);
        assert!(collect(&fixture).is_err(), "{mode}");
        assert_eq!(snapshot(&fixture.root), before);
    }
}

#[test]
fn complete_conflicting_history_is_refused_by_existing_strict_validator() {
    let fixture = semantic::fixture();
    semantic::authority(&fixture.config, 1, &semantic::add());
    semantic::authority(&fixture.config, 2, &semantic::remove());
    semantic::authority(&fixture.config, 3, &semantic::add());
    assert_eq!(
        collect(&fixture).unwrap_err().code,
        "mutation_ledger_migration_invalid"
    );
}

#[test]
fn source_change_after_validation_or_projection_discards_buffered_output() {
    for stage in [Boundary::Validated, Boundary::Projected] {
        for mode in 0..3 {
            let fixture = semantic::fixture();
            let path = semantic::journal(
                &fixture.config,
                &Record::started(&fixture.config, &semantic::add()).unwrap(),
            );
            let result = evidence::collect(
                &fixture.config,
                &mut Budget::new(Limits::default()),
                16 * 1024 * 1024,
                fixture_mount,
                |at| {
                    if at == stage {
                        match mode {
                            0 => std::fs::write(&path, b"changed").unwrap(),
                            1 => {
                                fixture.write("mutation-authority/.authority-1.tmp", b"new");
                            }
                            _ => std::fs::remove_file(&path).unwrap(),
                        }
                    }
                    Ok(())
                },
            );
            assert!(result.is_err(), "mode {mode}");
        }
    }
}

#[test]
fn symlink_and_fifo_sources_are_refused_without_following_or_blocking() {
    for fifo in [false, true] {
        let fixture = semantic::fixture();
        let path = semantic::journal(
            &fixture.config,
            &Record::started(&fixture.config, &semantic::add()).unwrap(),
        );
        std::fs::remove_file(&path).unwrap();
        if fifo {
            let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
            // SAFETY: name is a valid path to the test-owned absent entry.
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        } else {
            std::os::unix::fs::symlink(fixture.root.join("missing"), &path).unwrap();
        }
        assert!(collect(&fixture).is_err());
    }
}

#[test]
fn all_passes_share_entry_byte_and_metadata_limits() {
    let fixture = semantic::fixture();
    semantic::journal(
        &fixture.config,
        &Record::started(&fixture.config, &semantic::add()).unwrap(),
    );
    let mut observed = Budget::new(Limits::default());
    let expected = evidence::collect(
        &fixture.config,
        &mut observed,
        16 * 1024 * 1024,
        fixture_mount,
        |_| Ok(()),
    )
    .unwrap();
    for mode in 0..3 {
        let mut limits = Limits::default();
        match mode {
            0 => limits.entries = observed.entries,
            1 => limits.bytes = observed.bytes,
            _ => limits.metadata_bytes = observed.metadata_bytes,
        }
        assert_eq!(
            evidence::collect(
                &fixture.config,
                &mut Budget::new(limits),
                16 * 1024 * 1024,
                fixture_mount,
                |_| Ok(())
            )
            .unwrap(),
            expected
        );
        match mode {
            0 => limits.entries -= 1,
            1 => limits.bytes -= 1,
            _ => limits.metadata_bytes -= 1,
        }
        assert_eq!(
            evidence::collect(
                &fixture.config,
                &mut Budget::new(limits),
                16 * 1024 * 1024,
                fixture_mount,
                |_| Ok(())
            )
            .unwrap_err()
            .code,
            "mutation_ledger_migration_capacity"
        );
    }
}

#[test]
fn elapsed_and_output_budgets_refuse_without_partial_success() {
    let fixture = semantic::fixture();
    let expected = collect(&fixture).unwrap();
    assert_eq!(
        evidence::collect(
            &fixture.config,
            &mut Budget::new(Limits::default()),
            expected.len(),
            fixture_mount,
            |_| Ok(())
        )
        .unwrap(),
        expected
    );
    assert_eq!(
        evidence::collect(
            &fixture.config,
            &mut Budget::new(Limits::default()),
            expected.len() - 1,
            fixture_mount,
            |_| Ok(())
        )
        .unwrap_err()
        .code,
        "ledger_evidence_output_budget"
    );
    let mut budget = Budget::new(Limits::default());
    budget.start = Instant::now() - Duration::from_secs(301);
    assert_eq!(
        evidence::collect(
            &fixture.config,
            &mut budget,
            16 * 1024 * 1024,
            fixture_mount,
            |_| Ok(())
        )
        .unwrap_err()
        .code,
        "mutation_ledger_migration_capacity"
    );
}

#[test]
fn file_mount_failure_does_not_count_as_absence() {
    let fixture = semantic::fixture();
    fn denied(file: &File) -> Result<()> {
        if file.metadata().unwrap().is_file() {
            Err(RuntimeError::rejected("fixture_mount_io", "unavailable"))
        } else {
            Ok(())
        }
    }
    assert_eq!(
        evidence::collect(
            &fixture.config,
            &mut Budget::new(Limits::default()),
            16 * 1024 * 1024,
            denied,
            |_| Ok(())
        )
        .unwrap_err()
        .code,
        "fixture_mount_io"
    );
}
