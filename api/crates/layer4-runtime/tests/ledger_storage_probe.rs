//! Qualify the shipped executable without opening any pre-existing state.
#[test]
fn create_only_probe_reports_the_actual_bundled_sqlite_and_refuses_existing_state() {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("cma-sqlite-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let database = root.join("probe");
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_layer4-runtime"))
            .args([
                std::ffi::OsStr::new("--verify-ledger-storage"),
                database.as_os_str(),
            ])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["sqlite_version"], rusqlite::version());
    assert!(report["sqlite_source_id"].as_str().unwrap().len() > 64);
    assert!(report["compile_options"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "THREADSAFE=1"));
    assert_eq!(report["journal_mode"], "delete");
    assert_eq!(report["synchronous"], "extra");
    assert_eq!(report["create_commit_reopen"], "passed");
    assert_eq!(report["anchored_import_create_commit_reopen"], "passed");
    assert_eq!(report["retirement_archive_copy_reopen_refusal"], "passed");
    let path = database.join("ledger.sqlite3");
    let before = std::fs::read(&path).unwrap();
    assert!(!run().status.success());
    assert_eq!(std::fs::read(path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}
