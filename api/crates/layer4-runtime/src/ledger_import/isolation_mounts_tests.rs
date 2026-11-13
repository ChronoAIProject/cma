use super::*;
use std::cell::Cell;

struct Fixture {
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("ledger-mount-proof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let home = root.join("protected");
        std::fs::create_dir(&home).unwrap();
        Self { root, home }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn record(root: &str, path: &std::path::Path) -> String {
    format!("42 1 1:2 {root} {} rw - ext4 none rw\n", path.display())
}

#[test]
fn disappearing_mountpoint_rechecks_topology_before_refusing_plan_isolation() {
    let fixture = Fixture::new();
    let mount = fixture.root.join("unrelated-mount");
    std::fs::create_dir(&mount).unwrap();
    let proof = Proof::new(&[&fixture.home]).unwrap();
    let stale = record("/", &mount);
    let stable = record("/", std::path::Path::new("/"));
    let reads = Cell::new(0);
    let checks = Cell::new(0);
    let result = consistent(
        || {
            let read = reads.get();
            reads.set(read + 1);
            if read == 0 {
                // Deterministic unmount+mountpoint removal after /proc was read,
                // before the first pathname inspection. No privileged mount.
                std::fs::remove_dir(&mount).unwrap();
                Ok(stale.clone())
            } else {
                Ok(stable.clone())
            }
        },
        |text| {
            checks.set(checks.get() + 1);
            proof.mount_layout(text, &[(1, 2)], (1, 2))
        },
    );
    result.unwrap();
    assert_eq!(checks.get(), 2);
    assert_eq!(reads.get(), 4);
    assert!(!fixture.root.join("output").exists());
}

#[test]
fn stable_bind_nested_and_missing_mountpoints_still_refuse() {
    let fixture = Fixture::new();
    let protected = &fixture.home;
    let nested = protected.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let proof = Proof::new(&[protected]).unwrap();
    let missing = fixture.root.join("missing");
    for text in [
        record("/subtree", &fixture.root),
        record("/", &nested),
        record("/", &missing),
    ] {
        let reads = Cell::new(0);
        let result = consistent(
            || {
                reads.set(reads.get() + 1);
                Ok(text.clone())
            },
            |text| proof.mount_layout(text, &[(1, 2)], (1, 2)),
        );
        assert_eq!(
            result.unwrap_err().code,
            "mutation_ledger_migration_invalid"
        );
        assert_eq!(reads.get(), 2);
    }
    assert!(!fixture.root.join("output").exists());
}

#[test]
fn new_alias_after_success_is_rechecked_and_refused() {
    let fixture = Fixture::new();
    let proof = Proof::new(&[&fixture.home]).unwrap();
    let reads = Cell::new(0);
    let result = consistent(
        || {
            let read = reads.get();
            reads.set(read + 1);
            Ok(record(
                if read == 0 { "/" } else { "/subtree" },
                &fixture.root,
            ))
        },
        |text| proof.mount_layout(text, &[(1, 2)], (1, 2)),
    );
    assert_eq!(
        result.unwrap_err().code,
        "mutation_ledger_migration_invalid"
    );
    assert_eq!(reads.get(), 4);
}

#[test]
fn continuous_topology_changes_are_bounded_and_hold() {
    let reads = Cell::new(0);
    let result = consistent(
        || {
            reads.set(reads.get() + 1);
            Ok(reads.get())
        },
        |_| Ok(()),
    );
    assert_eq!(
        result.unwrap_err().code,
        "mutation_ledger_migration_changed"
    );
    assert_eq!(reads.get(), 6);
}

#[test]
fn failed_topology_read_cannot_certify_a_successful_inspection() {
    let reads = Cell::new(0);
    let result = consistent(
        || {
            reads.set(reads.get() + 1);
            if reads.get() == 1 {
                Ok(())
            } else {
                Err(invalid())
            }
        },
        |_| Ok(()),
    );
    assert_eq!(
        result.unwrap_err().code,
        "mutation_ledger_migration_invalid"
    );
}

#[test]
fn mount_paths_decode_only_proc_escapes() {
    assert_eq!(
        mount_path("/with\\040space\\134slash").unwrap(),
        PathBuf::from("/with space\\slash")
    );
    assert!(mount_path("/bad\\999escape").is_err());
}
