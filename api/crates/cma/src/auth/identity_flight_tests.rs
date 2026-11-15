use super::*;

#[test]
fn token_keys_are_process_keyed_and_distinct_without_retaining_credentials() {
    let a = MemoryIdentityStore::default();
    let b = MemoryIdentityStore::default();
    assert_eq!(a.key("first-fixture"), a.key("first-fixture"));
    assert_ne!(a.key("first-fixture"), a.key("second-fixture"));
    assert_ne!(a.key("first-fixture"), b.key("first-fixture"));
    assert_eq!(a.active_receipts(), 0);
}

#[test]
fn successful_publication_supersedes_only_the_exact_tokens_live_receipts() {
    let store = MemoryIdentityStore::default();
    let key = store.key("first-fixture");
    let other = store.key("second-fixture");
    let started = store.token_epoch(&key).unwrap();
    let same = store.token_epoch(&key).unwrap();
    let unrelated = store.token_epoch(&other).unwrap();
    assert!(Arc::ptr_eq(&started, &same));
    let completed = store.finish_verification(&key, &started).unwrap();
    assert!(!store.token_current(&key, &started));
    assert!(store.token_current(&key, &completed));
    assert!(store.token_current(&other, &unrelated));
    assert!(store.finish_verification(&key, &started).is_none());
    let replacement = store.finish_verification(&key, &completed).unwrap();
    assert!(!store.token_current(&key, &completed));
    assert!(store.token_current(&key, &replacement));
}

#[test]
fn refusal_and_logout_fence_pending_and_completed_live_consumers() {
    let store = MemoryIdentityStore::default();
    let key = store.key("revoked-fixture");
    let pending = store.token_epoch(&key).unwrap();
    store.invalidate_token("revoked-fixture");
    assert!(store.finish_verification(&key, &pending).is_none());
    let next = store.token_epoch(&key).unwrap();
    let completed = store.finish_verification(&key, &next).unwrap();
    store.clear();
    assert!(!store.token_current(&key, &completed));
}

#[test]
fn bounded_registry_releases_consumers_without_retaining_results() {
    let store = MemoryIdentityStore::new(1);
    let first = store.key("first-fixture");
    let second = store.key("second-fixture");
    let pending = store.token_epoch(&first).unwrap();
    assert!(store.token_epoch(&second).is_none());
    let completed = store.finish_verification(&first, &pending).unwrap();
    assert_eq!(store.active_receipts(), 1);
    drop(completed);
    assert_eq!(store.active_receipts(), 0);
    let fresh = store.token_epoch(&second).unwrap();
    assert!(store.token_current(&second, &fresh));
    assert!(!store.token_current(&first, &pending));
    assert_eq!(store.epochs.lock().unwrap().len(), 1);
    assert!(MemoryIdentityStore::new(0).token_epoch(&first).is_none());
}
