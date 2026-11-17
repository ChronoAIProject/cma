use super::*;

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cmaeg-capture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ordinary_replica_ledger_does_not_require_a_host_collector() {
    let ledger = Ledger::in_directory(None, 2, true);
    ledger.record(json!({"stage":"setup", "unique_events":1}));
    ledger.record(json!({"stage":"final", "failures":[], "complete":true}));
}

#[test]
fn configured_ledger_preserves_rows_and_refuses_existing_evidence() {
    let root = Root::new();
    let rows = [
        json!({"stage":"setup", "unique_events":1}),
        json!({"stage":"final", "failures":[], "complete":true}),
    ];
    let ledger = Ledger::in_directory(Some(&root.0), 2, true);
    for row in &rows {
        ledger.record(row.clone());
    }
    drop(ledger);
    let path = root.0.join("n2-true-ledger.jsonl");
    let before = std::fs::read_to_string(&path).unwrap();
    let recorded: Vec<Value> = before
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(recorded, rows);
    assert!(std::panic::catch_unwind(|| { Ledger::in_directory(Some(&root.0), 2, true) }).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn configured_ledger_refuses_an_unavailable_root() {
    let root = Root::new();
    let missing = root.0.join("missing");
    assert!(
        std::panic::catch_unwind(|| { Ledger::in_directory(Some(&missing), 2, true) }).is_err()
    );
    assert!(!missing.exists());
}
