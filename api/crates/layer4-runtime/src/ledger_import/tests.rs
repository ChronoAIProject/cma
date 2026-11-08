use super::*;
use crate::config::RuntimeConfig;
use std::path::{Path, PathBuf};
#[path = "plan_tests.rs"]
mod plans;
#[path = "sealed_tests.rs"]
mod sealed_plans;
#[path = "validation_tests.rs"]
mod semantic;
#[path = "stage_tests.rs"]
mod staging;

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("cma-ledger-inventory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.state_root = root.join("state");
        config.checkpoint_root = root.join("transfer");
        config.home_identity = Some(format!("sha256:{}", "a".repeat(64)));
        config.generation = 7;
        for directory in [&config.home, &config.state_root, &config.checkpoint_root] {
            std::fs::create_dir_all(directory).unwrap();
        }
        Self { root, config }
    }
    fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.config.state_root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn scan(&self) -> Result<inventory::Inventory> {
        inventory::scan(&self.config, Limits::default())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn name(index: usize) -> String {
    format!("{index:064x}.json")
}

#[test]
fn observes_exhausted_and_historical_scopes_without_recovering_temporaries() {
    let fixture = Fixture::new();
    for index in 0..1025 {
        fixture.write(
            &format!("runtime-mutations/{}", name(index)),
            b"journal bytes",
        );
    }
    for index in 0..4097 {
        fixture.write(
            &format!("mutation-authority/{}", name(index)),
            b"authority bytes",
        );
    }
    let partial = fixture.write("mutation-authority/.authority-1-1.tmp", b"{");
    fixture.write(
        &format!("process-generation-2/runtime-mutations/{}", name(1)),
        b"prior scope",
    );
    fixture.write(
        "workspace-operations/.workspace-journal-1-1.tmp",
        b"untouched",
    );
    std::fs::create_dir_all(fixture.config.checkpoint_root.join("runtime-state")).unwrap();
    std::fs::write(
        fixture
            .config
            .checkpoint_root
            .join("runtime-state/.install-1.tmp"),
        b"installation",
    )
    .unwrap();
    let inventory = fixture.scan().unwrap();
    assert_eq!(inventory.entries.len(), 5126);
    assert_eq!(std::fs::read(partial).unwrap(), b"{");
    assert!(inventory.roots.iter().any(|root| root.scope == 2));
    let summary = inventory.summary();
    assert_eq!(summary["semantic_validation"], "pending");
    assert_eq!(summary["writer_exclusion"], "unproved");
    assert_eq!(summary["temporary_files"], 3);
    let text = summary.to_string();
    assert!(!text.contains(&fixture.config.sandbox_id));
    assert!(!text.contains(fixture.root.to_str().unwrap()));
    assert!(!text.contains("journal bytes"));
    assert_eq!(inventory.summary(), fixture.scan().unwrap().summary());
}

#[test]
fn separate_budgets_stop_inspection_without_changing_input() {
    let fixture = Fixture::new();
    let source = fixture.write(&format!("runtime-mutations/{}", name(1)), b"unchanged");
    for limits in [
        Limits {
            entries: 1,
            ..Limits::default()
        },
        Limits {
            bytes: 3,
            ..Limits::default()
        },
        Limits {
            metadata_bytes: 10,
            ..Limits::default()
        },
        Limits {
            deadline: Duration::ZERO,
            ..Limits::default()
        },
    ] {
        let error = inventory::scan(&fixture.config, limits).err().unwrap();
        assert_eq!(error.code, "mutation_ledger_migration_capacity");
        assert!(!error.retryable);
        assert!(error.detail.contains("_at_least="));
        assert!(!error.detail.contains(fixture.root.to_str().unwrap()));
        assert_eq!(std::fs::read(&source).unwrap(), b"unchanged");
    }
}

#[test]
fn refusal_lower_bounds_report_only_the_exhausted_budget() {
    let mut budget = Budget::new(Limits {
        entries: 1,
        bytes: 3,
        metadata_bytes: 4096,
        ..Limits::default()
    });
    budget.entry(1).unwrap();
    assert!(budget
        .entry(1)
        .unwrap_err()
        .detail
        .ends_with("examined_entries_at_least=2"));
    assert!(budget
        .file(4)
        .unwrap_err()
        .detail
        .ends_with("requested_read_bytes_at_least=4"));
    let mut metadata = Budget::new(Limits {
        metadata_bytes: 1,
        ..Limits::default()
    });
    assert!(metadata
        .path(3)
        .unwrap_err()
        .detail
        .ends_with("metadata_bytes_at_least=518"));
    assert!(json::check(&serde_json::to_vec(&vec![0; 4096]).unwrap())
        .unwrap_err()
        .detail
        .ends_with("json_nodes_at_least=4097"));
}

#[test]
fn refuses_unknown_names_and_invalid_generations_without_v1_fallback() {
    for relative in [
        "runtime-mutations/unknown",
        "mutation-authority/unknown",
        "home-binding/unknown",
        "process-generation-0/runtime-mutations/a",
        "process-generation-08/runtime-mutations/a",
        "process-generation-8/runtime-mutations/a",
        "process-generation-x/runtime-mutations/a",
        "process-generation-2/unknown",
        "mutation-ledger-v2.current.json",
        "workspace-prepare-recovery/a",
        "workspace-removal-current-v1/a",
    ] {
        let fixture = Fixture::new();
        let path = fixture.write(relative, b"original");
        assert_eq!(
            fixture.scan().err().unwrap().code,
            "mutation_ledger_migration_invalid",
            "{relative}"
        );
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
}

#[test]
fn refuses_links_in_ancestors_directories_and_files_and_nonregular_entries() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = fixture.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let directory = fixture.config.state_root.join("runtime-mutations");
    symlink(&outside, &directory).unwrap();
    assert!(fixture.scan().is_err());
    std::fs::remove_file(&directory).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join(name(1));
    std::fs::write(outside.join("data"), b"outside").unwrap();
    symlink(outside.join("data"), &path).unwrap();
    assert!(fixture.scan().is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(fixture.scan().is_err());
    let alias = fixture.root.join("alias");
    symlink(&fixture.config.state_root, &alias).unwrap();
    assert!(io::Directory::open(&alias.join("runtime-mutations")).is_err());
    assert!(io::Directory::open(Path::new("relative")).is_err());
    assert!(io::Directory::open(&fixture.config.state_root.join("..")).is_err());
    assert_eq!(std::fs::read(outside.join("data")).unwrap(), b"outside");
}

#[test]
fn rejects_oversized_files_before_allocating_the_payload() {
    let fixture = Fixture::new();
    let path = fixture.write(&format!("mutation-authority/{}", name(1)), b"");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(crate::mutation_authority::MAX_AUTHORITY_BYTES + 1)
        .unwrap();
    assert_eq!(
        fixture.scan().err().unwrap().code,
        "mutation_ledger_migration_invalid"
    );
}

#[test]
fn digest_changes_for_input_change_and_preserves_ownership_evidence() {
    let fixture = Fixture::new();
    let source = fixture.write("home-binding/binding.json", b"binding");
    fixture.write("workspace-removal-writer-v1.json", b"writer");
    fixture.write("workspace-removal-writer-v1.lock", b"");
    fixture.write(".workspace-removal-writer-1.tmp", b"partial");
    let before = fixture.scan().unwrap().summary();
    assert_eq!(before["files"], 4);
    std::fs::write(&source, b"changed").unwrap();
    let after = fixture.scan().unwrap().summary();
    assert_ne!(before["inventory_sha256"], after["inventory_sha256"]);
    assert_eq!(std::fs::read(source).unwrap(), b"changed");
}
