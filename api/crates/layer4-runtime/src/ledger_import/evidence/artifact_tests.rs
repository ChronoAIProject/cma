use super::*;
use crate::ledger_import::evidence::{self, Boundary};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

fn mount(_: &File) -> Result<()> {
    Ok(())
}

fn source(fixture: &Fixture) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &Path, rows: &mut BTreeMap<PathBuf, Vec<u8>>) {
        rows.insert(path.into(), b"directory".to_vec());
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, rows);
            } else {
                rows.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
    let mut rows = BTreeMap::new();
    for path in [
        &fixture.config.home,
        &fixture.config.state_root,
        &fixture.config.checkpoint_root,
    ] {
        visit(path, &mut rows);
    }
    rows
}

fn create(fixture: &Fixture, path: &Path) -> Result<evidence::Receipt> {
    evidence::create(
        &fixture.config,
        path,
        &mut Budget::new(Limits::default()),
        16 * 1024 * 1024,
        mount,
        |_| Ok(()),
    )
}

#[test]
fn private_artifact_and_safe_receipt_bind_the_complete_payload() {
    use sha2::{Digest, Sha256};
    let fixture = semantic::fixture();
    semantic::journal(
        &fixture.config,
        &crate::mutation_journal::Record::started(&fixture.config, &semantic::add()).unwrap(),
    );
    let before = source(&fixture);
    let destination = fixture.root.join("evidence");
    let receipt = serde_json::to_value(create(&fixture, &destination).unwrap()).unwrap();
    let bytes = std::fs::read(destination.join("evidence.json")).unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let sealed = std::fs::read(destination.join("sealed.json")).unwrap();
    let seal: serde_json::Value = serde_json::from_slice(&sealed).unwrap();
    assert_eq!(receipt["payload_bytes"], bytes.len());
    assert_eq!(
        receipt["payload_sha256"],
        crate::command::hex(&Sha256::digest(&bytes))
    );
    assert_eq!(
        receipt["sealed_sha256"],
        crate::command::hex(&Sha256::digest(&sealed))
    );
    assert_eq!(receipt["payload_sha256"], seal["payload_sha256"]);
    assert_eq!(
        receipt["source_records"],
        payload["records"].as_array().unwrap().len()
    );
    assert_eq!(seal["inventory_sha256"], payload["inventory_sha256"]);
    assert_eq!(seal["restore_admission"], "HELD");
    assert_eq!(receipt["semantic_reconciliation"], "NOT_RUN");
    assert!(payload["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["scope"]["kind"] == "legacy_global" && r["scope"].get("generation").is_none()));
    let safe = receipt.to_string();
    for private in [
        &fixture.config.sandbox_id,
        fixture.config.home_identity.as_ref().unwrap(),
        &semantic::add().operation_id,
    ] {
        assert!(!safe.contains(private));
    }
    assert!(!safe.contains(fixture.root.to_str().unwrap()));
    assert!(safe.len() < 4096);
    assert!(sealed.len() <= 4096);
    assert_eq!(
        std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for name in ["evidence.json", "sealed.json"] {
        assert_eq!(
            std::fs::metadata(destination.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }
    assert_eq!(source(&fixture), before);
    assert!(create(&fixture, &destination).is_err());
    assert_eq!(
        std::fs::read(destination.join("evidence.json")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(destination.join("sealed.json")).unwrap(),
        sealed
    );
}

#[test]
fn output_exhaustion_leaves_no_completion_and_preserves_sources() {
    let fixture = semantic::fixture();
    let before = source(&fixture);
    let destination = fixture.root.join("evidence");
    let error = evidence::create(
        &fixture.config,
        &destination,
        &mut Budget::new(Limits::default()),
        1,
        mount,
        |_| Ok(()),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "ledger_evidence_output_budget");
    assert!(!destination.join("sealed.json").exists());
    assert!(
        std::fs::read(destination.join("evidence.json"))
            .unwrap()
            .len()
            <= 1
    );
    assert_eq!(source(&fixture), before);
}

#[test]
fn source_drift_after_projection_or_payload_sync_prevents_completion() {
    for stage in [Boundary::Projected, Boundary::PayloadStored] {
        let fixture = semantic::fixture();
        std::fs::create_dir_all(fixture.config.state_root.join("mutation-authority")).unwrap();
        let before = source(&fixture);
        let destination = fixture.root.join("evidence");
        let error = evidence::create(
            &fixture.config,
            &destination,
            &mut Budget::new(Limits::default()),
            16 * 1024 * 1024,
            mount,
            |at| {
                if at == stage {
                    fixture.write(
                        "mutation-authority/.authority-1.tmp",
                        b"new unpublished bytes",
                    );
                }
                Ok(())
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "mutation_ledger_migration_changed");
        assert!(!destination.join("sealed.json").exists());
        std::fs::remove_file(
            fixture
                .config
                .state_root
                .join("mutation-authority/.authority-1.tmp"),
        )
        .unwrap();
        assert_eq!(source(&fixture), before);
    }
}

#[test]
fn private_publication_uses_the_same_cumulative_budget_through_final_scan() {
    let fixture = semantic::fixture();
    let before = source(&fixture);
    let mut measured = Budget::new(Limits::default());
    evidence::create(
        &fixture.config,
        &fixture.root.join("measured"),
        &mut measured,
        16 * 1024 * 1024,
        mount,
        |_| Ok(()),
    )
    .unwrap();
    for kind in 0..3 {
        let mut limits = Limits::default();
        match kind {
            0 => limits.bytes = measured.bytes,
            1 => limits.entries = measured.entries,
            _ => limits.metadata_bytes = measured.metadata_bytes,
        }
        evidence::create(
            &fixture.config,
            &fixture.root.join(format!("exact-{kind}")),
            &mut Budget::new(limits),
            16 * 1024 * 1024,
            mount,
            |_| Ok(()),
        )
        .unwrap();
        match kind {
            0 => limits.bytes -= 1,
            1 => limits.entries -= 1,
            _ => limits.metadata_bytes -= 1,
        }
        let destination = fixture.root.join(format!("below-{kind}"));
        let error = evidence::create(
            &fixture.config,
            &destination,
            &mut Budget::new(limits),
            16 * 1024 * 1024,
            mount,
            |_| Ok(()),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "mutation_ledger_migration_capacity");
        assert!(!destination.join("sealed.json").exists());
    }
    assert_eq!(source(&fixture), before);
}

#[test]
fn projection_cannot_reset_the_aggregate_real_input_allowance() {
    let fixture = semantic::fixture();
    let before = source(&fixture);
    let bytes = std::fs::metadata(fixture.config.state_root.join("home-binding/binding.json"))
        .unwrap()
        .len();
    // Initial scan, validation, projection, and both final scans each read
    // this real binding once, even though every individual read fits alone.
    let limits = Limits {
        bytes: bytes * 5,
        ..Limits::default()
    };
    let mut exact = Budget::new(limits);
    evidence::create(
        &fixture.config,
        &fixture.root.join("exact"),
        &mut exact,
        16 * 1024 * 1024,
        mount,
        |_| Ok(()),
    )
    .unwrap();
    let destination = fixture.root.join("one-byte-short");
    let error = match evidence::create(
        &fixture.config,
        &destination,
        &mut Budget::new(Limits {
            bytes: limits.bytes - 1,
            ..limits
        }),
        16 * 1024 * 1024,
        mount,
        |_| Ok(()),
    ) {
        Ok(_) => panic!("aggregate input allowance admitted an over-budget artifact"),
        Err(error) => error,
    };
    assert_eq!(error.code, "mutation_ledger_migration_capacity");
    assert!(error
        .detail
        .contains(&format!("requested_read_bytes_at_least={}", bytes * 5)));
    assert_eq!(exact.bytes, bytes * 5);
    assert!(!destination.join("sealed.json").exists());
    assert_eq!(source(&fixture), before);
}

#[test]
fn protected_or_replaced_output_is_never_accepted() {
    let fixture = semantic::fixture();
    let before = source(&fixture);
    for path in [
        fixture.config.home.join("output"),
        fixture.config.state_root.join("output"),
        fixture.config.checkpoint_root.join("output"),
    ] {
        assert!(create(&fixture, &path).is_err());
        assert!(!path.exists());
    }
    let destination = fixture.root.join("evidence");
    let moved = fixture.root.join("moved");
    let error = evidence::create(
        &fixture.config,
        &destination,
        &mut Budget::new(Limits::default()),
        16 * 1024 * 1024,
        mount,
        |at| {
            if at == Boundary::PayloadStored {
                std::fs::rename(&destination, &moved).unwrap();
                std::fs::create_dir(&destination).unwrap();
                std::fs::write(destination.join("sentinel"), b"preserve replacement").unwrap();
            }
            Ok(())
        },
    )
    .err()
    .unwrap();
    assert_eq!(error.code, "mutation_ledger_migration_changed");
    assert_eq!(
        std::fs::read(destination.join("sentinel")).unwrap(),
        b"preserve replacement"
    );
    assert!(!destination.join("sealed.json").exists());
    assert!(!moved.join("sealed.json").exists());
    assert_eq!(source(&fixture), before);
}

#[test]
fn short_writes_and_flush_failure_keep_exact_output_outcomes() {
    struct Sink {
        bytes: Vec<u8>,
        remaining: usize,
        fail_flush: bool,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            let n = bytes.len().min(3).min(self.remaining);
            self.bytes.extend_from_slice(&bytes[..n]);
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                Err(std::io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
    }
    let fixture = semantic::fixture();
    let before = source(&fixture);
    let expected = evidence::collect(
        &fixture.config,
        &mut Budget::new(Limits::default()),
        16 * 1024 * 1024,
        mount,
        |_| Ok(()),
    )
    .unwrap();
    for (remaining, fail_flush) in [(usize::MAX, false), (33, false), (usize::MAX, true)] {
        let mut sink = Sink {
            bytes: Vec::new(),
            remaining,
            fail_flush,
        };
        let result = evidence::collect_to(
            &fixture.config,
            &mut Budget::new(Limits::default()),
            16 * 1024 * 1024,
            mount,
            |_| Ok(()),
            &mut sink,
        );
        if remaining == 33 || fail_flush {
            assert_eq!(result.unwrap_err().code, "ledger_evidence_output_io");
        } else {
            result.unwrap();
        }
        if remaining == 33 {
            assert_eq!(sink.bytes, expected[..33]);
        } else {
            assert_eq!(sink.bytes, expected);
        }
        assert_eq!(source(&fixture), before);
    }
}

#[test]
fn generation_and_fence_identity_remain_lossless_above_json_safe_integer_range() {
    let mut fixture = semantic::fixture();
    fixture.config.generation = u64::MAX;
    let binding = fixture.config.state_root.join("home-binding/binding.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&binding).unwrap()).unwrap();
    value["runtime_generation"] = u64::MAX.into();
    std::fs::write(binding, serde_json::to_vec(&value).unwrap()).unwrap();
    semantic::authority(&fixture.config, u64::MAX, &semantic::add());
    let operation =
        crate::mutation_journal::Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
            "agent:agent:close:v1",
            layer3_adapter::AgentSlot::new("agent"),
        ));
    semantic::journal(
        &fixture.config,
        &crate::mutation_journal::Record::started(&fixture.config, &operation).unwrap(),
    );
    let before = source(&fixture);
    let destination = fixture.root.join("evidence");
    create(&fixture, &destination).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(destination.join("evidence.json")).unwrap()).unwrap();
    let exact = "18446744073709551615";
    assert_eq!(value["generation"], exact);
    let records = value["records"].as_array().unwrap();
    assert!(records.iter().any(|r| r["fact"]["fence"] == exact));
    assert!(records.iter().any(|r| r["scope"]["generation"] == exact));
    assert_eq!(source(&fixture), before);
}
