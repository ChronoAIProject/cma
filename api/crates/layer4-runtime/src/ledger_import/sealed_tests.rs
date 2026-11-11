use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

fn plan() -> (Fixture, PathBuf, String) {
    let fixture = super::plans::fixture();
    let path = fixture.root.join("sealed");
    let summary = plan::create(&fixture.config, &path).unwrap();
    let digest = summary["plan_sha256"].as_str().unwrap().to_owned();
    (fixture, path, digest)
}

fn replace(path: &Path, bytes: &[u8]) {
    std::fs::remove_file(path).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400)).unwrap();
}

#[test]
fn sealed_plan_reopens_without_source_files_and_never_releases_pending_work() {
    let (fixture, path, digest) = plan();
    let before = fixture.scan().unwrap().digest;
    let (plan, analysis) = sealed::open(&fixture.config, &path, &digest).unwrap();
    assert_eq!(analysis.summary(&plan.inventory)["started"], 1);
    assert_eq!(before, fixture.scan().unwrap().digest);
    std::fs::remove_dir_all(&fixture.config.state_root).unwrap();
    std::fs::remove_dir_all(&fixture.config.checkpoint_root).unwrap();
    let (plan, analysis) = sealed::open(&fixture.config, &path, &digest).unwrap();
    assert_eq!(plan.admission, "import_held");
    assert_eq!(plan.writer_exclusion, "unproved");
    assert_eq!(analysis.summary(&plan.inventory)["started"], 1);
    assert!(!fixture.config.state_root.exists());
    assert!(!fixture.config.checkpoint_root.exists());
}

#[test]
fn sealed_plan_requires_the_independent_digest_and_exact_source_binding() {
    let (fixture, path, digest) = plan();
    for incorrect in ["", "../path", &"b".repeat(64), &"F".repeat(64)] {
        assert!(sealed::open(&fixture.config, &path, incorrect).is_err());
    }
    for field in 0..6 {
        let mut config = fixture.config.clone();
        match field {
            0 => config.sandbox_id = "foreign".into(),
            1 => config.home_identity = Some("b".repeat(43)),
            2 => config.generation += 1,
            3 => config.home = fixture.root.join("another-home"),
            4 => config.state_root = fixture.root.join("another-state"),
            _ => config.checkpoint_root = fixture.root.join("another-transfer"),
        }
        assert!(sealed::open(&config, &path, &digest).is_err());
    }
    assert!(sealed::open(&fixture.config, &path, &digest).is_ok());
}

#[test]
fn sealed_plan_refuses_incomplete_tampered_extra_linked_or_writable_bytes() {
    for mode in 0..10 {
        let (fixture, path, digest) = plan();
        let record = path.join("00000000.record");
        match mode {
            0 => std::fs::remove_file(path.join("sealed.json")).unwrap(),
            1 => replace(&path.join("sealed.json"), b"{"),
            2 => replace(&path.join("plan.json"), b"{}"),
            3 => replace(&record, b"tampered"),
            4 => std::fs::write(path.join("extra"), b"extra").unwrap(),
            5 => std::fs::remove_file(&record).unwrap(),
            6 => {
                let outside = fixture.root.join("outside");
                std::fs::rename(&record, &outside).unwrap();
                symlink(outside, &record).unwrap();
            }
            7 => std::fs::hard_link(&record, fixture.root.join("linked")).unwrap(),
            8 => std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap(),
            _ => replace(&path.join("sealed.json"), b""),
        }
        assert!(
            sealed::open(&fixture.config, &path, &digest).is_err(),
            "mode {mode}"
        );
    }
}

#[test]
fn sealed_plan_rechecks_raw_files_on_each_read_and_refuses_changed_directory() {
    let (fixture, path, digest) = plan();
    let (plan, _) = sealed::open(&fixture.config, &path, &digest).unwrap();
    let mut budget = Budget::new(Limits::default());
    assert!(plan.inventory.read(0, &mut budget).is_ok());
    replace(&path.join("00000000.record"), b"drift");
    assert_eq!(
        plan.inventory.read(0, &mut budget).unwrap_err().code,
        "mutation_ledger_migration_changed"
    );
    assert!(plan.inventory.read(usize::MAX, &mut budget).is_err());
}

#[test]
fn manifest_budget_refuses_before_publishing_a_seal() {
    let fixture = super::plans::fixture();
    let path = fixture.root.join("bounded");
    let archive = plan_io::Archive::create(&fixture.config, &path).unwrap();
    assert_eq!(
        archive
            .json_bounded("plan.json", &"longer than the test budget", 4)
            .unwrap_err()
            .code,
        "mutation_ledger_migration_capacity"
    );
    assert!(!path.join("sealed.json").exists());
    assert!(std::fs::metadata(path.join("plan.json")).unwrap().len() <= 4);
}

#[test]
fn invalid_manifest_index_is_refused_without_panicking_or_reading_a_source_path() {
    use sha2::{Digest, Sha256};
    let (fixture, path, _) = plan();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("plan.json")).unwrap()).unwrap();
    manifest["inventory"]["entries"][0]["directory"] = usize::MAX.into();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let digest = crate::command::hex(&Sha256::digest(&bytes));
    replace(&path.join("plan.json"), &bytes);
    let mut seal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("sealed.json")).unwrap()).unwrap();
    seal["plan_sha256"] = digest.clone().into();
    replace(
        &path.join("sealed.json"),
        &serde_json::to_vec(&seal).unwrap(),
    );
    assert_eq!(
        sealed::open(&fixture.config, &path, &digest)
            .err()
            .unwrap()
            .code,
        "mutation_ledger_migration_invalid"
    );
}
