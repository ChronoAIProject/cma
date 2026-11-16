use super::ledger::{hash, Ledger};
use serde_json::{json, Value};

#[test]
fn acknowledged_operations_have_an_append_only_synced_journal_chain() {
    let path = std::env::temp_dir().join(format!("eg-ledger-{}.jsonl", uuid::Uuid::new_v4()));
    let mut ledger = Ledger::open(&path).unwrap();
    let body = json!({"conversation":"synthetic","metadata":{}});
    let bytes = body.to_string().into_bytes();
    ledger.admit("key", "event", &bytes, &body).unwrap();
    ledger.admit("key", "event", &bytes, &body).unwrap();
    ledger
        .effect("event", b"reply", "fixture terminal text")
        .unwrap();
    ledger.close().unwrap();
    ledger
        .effect("event", b"late-reply", "fixture terminal text")
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut previous = String::new();
    for (index, line) in text.lines().enumerate() {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["sequence"], index + 1);
        assert_eq!(value["previous_sha256"], previous);
        previous = hash(line.as_bytes());
    }
    assert_eq!(text.lines().count(), 6);
    assert_eq!(ledger.snapshot()["journal_sha256"], previous);
    assert!(ledger.effects[1].after_close);
    assert_eq!(ledger.responses.len(), 1);
    assert!(Ledger::open(&path).is_err());
    drop(ledger);
    std::fs::remove_file(path).unwrap();
}

#[cfg(unix)]
#[test]
fn existing_paths_and_symlinks_are_never_overwritten() {
    use std::os::unix::fs::symlink;
    // A directory cannot become an append-only file; an existing path is never
    // overwritten. Runtime journal I/O failures additionally poison the ledger.
    let path = std::env::temp_dir().join(format!("eg-ledger-{}", uuid::Uuid::new_v4()));
    symlink("/dev/null", &path).unwrap();
    assert!(Ledger::open(&path).is_err());
    std::fs::remove_file(path).unwrap();
}
