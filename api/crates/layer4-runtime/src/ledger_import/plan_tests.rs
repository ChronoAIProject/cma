use super::*;
use crate::mutation_journal::{Operation, Record};
use layer3_adapter::WorktreeRequest;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{symlink, MetadataExt};

fn fixture() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(43));
    crate::persistent_home::initialize(&fixture.config).unwrap();
    let operation = Operation::add_worktree(&WorktreeRequest::new(
        "worktree:workspace:agent:add:v1",
        "workspace",
        "agent",
        "a".repeat(40),
    ));
    let record = Record::started(&fixture.config, &operation).unwrap();
    let path = record.inventory_name(&fixture.config);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, record.encode().unwrap()).unwrap();
    fixture.write("mutation-authority/.authority-1.tmp", b"unpublished");
    fixture
}

#[test]
fn sealed_plan_preserves_all_raw_bytes_and_source_identity_without_release() {
    let fixture = fixture();
    let before = fixture.scan().unwrap();
    let destination = fixture.root.join("plan");
    let summary = plan::create(&fixture.config, &destination).unwrap();
    assert_eq!(summary["backup"], "sealed");
    assert_eq!(summary["writer_exclusion"], "unproved");
    assert_eq!(summary["admission"], "held");
    assert_eq!(before.digest, fixture.scan().unwrap().digest);
    let manifest = std::fs::read(destination.join("plan.json")).unwrap();
    let seal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(destination.join("sealed.json")).unwrap()).unwrap();
    assert_eq!(
        seal["plan_sha256"],
        crate::command::hex(&Sha256::digest(&manifest))
    );
    let manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(manifest["admission"], "import_held");
    assert_eq!(manifest["generation"], fixture.config.generation);
    assert_eq!(manifest["sandbox"], fixture.config.sandbox_id);
    for (index, entry) in before.entries.iter().enumerate() {
        let copy = destination.join(format!("{index:08}.record"));
        let source = before.roots[entry.directory].path.join(&entry.name);
        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(&copy).unwrap()
        );
        assert_ne!(
            std::fs::metadata(&source).unwrap().ino(),
            std::fs::metadata(&copy).unwrap().ino()
        );
        assert_eq!(std::fs::metadata(copy).unwrap().mode() & 0o777, 0o400);
    }
    assert_eq!(
        std::fs::metadata(&destination).unwrap().mode() & 0o777,
        0o700
    );
    assert!(!summary.to_string().contains(&fixture.config.sandbox_id));
    assert!(!summary.to_string().contains(fixture.root.to_str().unwrap()));
    assert!(!fixture
        .config
        .state_root
        .join("mutation-ledger-v2.current.json")
        .exists());
}

#[test]
fn plan_refuses_source_aliases_existing_outputs_and_symlinked_ancestors() {
    let fixture = fixture();
    let original = fixture.scan().unwrap().digest;
    let existing = fixture.root.join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("keep"), b"keep").unwrap();
    let alias = fixture.root.join("alias");
    symlink(&existing, &alias).unwrap();
    for path in [
        fixture.config.state_root.join("plan"),
        fixture.config.home.join("plan"),
        fixture.config.checkpoint_root.join("plan"),
        fixture.root.clone(),
        existing.clone(),
        alias.join("plan"),
    ] {
        assert!(plan::create(&fixture.config, &path).is_err());
    }
    assert_eq!(std::fs::read(existing.join("keep")).unwrap(), b"keep");
    assert_eq!(original, fixture.scan().unwrap().digest);
}

#[test]
fn interrupted_plan_never_seals_partial_backup_or_changes_source() {
    let fixture = fixture();
    let before = fixture.scan().unwrap();
    for fail_at in 0..before.entries.len() + 2 {
        let destination = fixture.root.join(format!("interrupted-{fail_at}"));
        let mut step = 0;
        let result = plan::create_checked(&fixture.config, &destination, |_| {
            let fail = step == fail_at;
            step += 1;
            if fail {
                Err(invalid())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert!(!destination.join("sealed.json").exists());
        assert_eq!(before.digest, fixture.scan().unwrap().digest);
        // Retrying never treats partial output as a valid existing plan.
        assert!(plan::create(&fixture.config, &destination).is_err());
    }
}

#[test]
fn plan_rescan_refuses_changed_or_added_source_after_copy() {
    for add in [false, true] {
        let fixture = fixture();
        let destination = fixture.root.join("drift");
        let result = plan::create_checked(&fixture.config, &destination, |stage| {
            if matches!(stage, plan::Boundary::Seal) {
                let name = if add {
                    ".authority-2.tmp"
                } else {
                    ".authority-1.tmp"
                };
                fixture.write(&format!("mutation-authority/{name}"), b"changed");
            }
            Ok(())
        });
        assert_eq!(
            result.unwrap_err().code,
            "mutation_ledger_migration_changed"
        );
        assert!(!destination.join("sealed.json").exists());
    }
}

#[test]
fn plan_refuses_replaced_output_directory_before_publishing_seal() {
    let fixture = fixture();
    let destination = fixture.root.join("plan");
    let original = fixture.scan().unwrap().digest;
    let result = plan::create_checked(&fixture.config, &destination, |stage| {
        if matches!(stage, plan::Boundary::Seal) {
            std::fs::rename(&destination, fixture.root.join("detached")).unwrap();
            std::fs::create_dir(&destination).unwrap();
        }
        Ok(())
    });
    assert_eq!(
        result.unwrap_err().code,
        "mutation_ledger_migration_changed"
    );
    assert!(!destination.join("sealed.json").exists());
    assert!(!fixture.root.join("detached/sealed.json").exists());
    assert_eq!(original, fixture.scan().unwrap().digest);
}
