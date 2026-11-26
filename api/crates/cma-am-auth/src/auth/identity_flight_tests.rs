use super::super::identity_evidence::tests::fixture_stamp;
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
fn equal_publication_preserves_only_the_exact_tokens_live_receipts() {
    let store = MemoryIdentityStore::default();
    let key = store.key("first-fixture");
    let other = store.key("second-fixture");
    let stamp = fixture_stamp(&store, &key, &["use"]);
    let changed = fixture_stamp(&store, &key, &[]);
    let started = store.token_epoch(&key).unwrap();
    let same = store.token_epoch(&key).unwrap();
    let unrelated = store.token_epoch(&other).unwrap();
    assert!(Arc::ptr_eq(&started, &same));
    let completed = store.finish_verification(&key, &started, &stamp).unwrap();
    let second = store.finish_verification(&key, &same, &stamp).unwrap();
    assert!(Arc::ptr_eq(&started, &completed));
    assert!(Arc::ptr_eq(&completed, &second));
    assert!(store.token_current(&other, &unrelated));
    let replacement = store
        .finish_verification(&key, &completed, &changed)
        .unwrap();
    assert!(!store.token_current(&key, &completed));
    assert!(store.token_current(&key, &replacement));
    assert!(store.finish_verification(&key, &started, &stamp).is_none());
    assert!(store
        .finish_verification(&key, &started, &changed)
        .is_none());
    assert!(store.token_current(&other, &unrelated));
}

#[test]
fn both_publisher_orders_and_authority_round_trips_fence_stale_captures() {
    for reversed in [false, true] {
        let store = MemoryIdentityStore::default();
        let key = store.key("fixture");
        let a = fixture_stamp(&store, &key, &["use"]);
        let b = fixture_stamp(&store, &key, &[]);
        let (first, different) = if reversed { (&b, &a) } else { (&a, &b) };
        let pending_a = store.token_epoch(&key).unwrap();
        let pending_b = store.token_epoch(&key).unwrap();
        let completed_a = store.finish_verification(&key, &pending_a, first).unwrap();
        let completed_b = store
            .finish_verification(&key, &pending_b, different)
            .unwrap();
        assert!(!store.token_current(&key, &completed_a));
        assert!(store.finish_verification(&key, &pending_a, first).is_none());
        let returned = store
            .finish_verification(&key, &completed_b, first)
            .unwrap();
        assert!(!store.token_current(&key, &completed_a));
        assert!(!store.token_current(&key, &completed_b));
        assert!(store.token_current(&key, &returned));
        assert!(store
            .finish_verification(&key, &completed_a, first)
            .is_none());
    }
}

#[test]
fn refusal_and_logout_cannot_republish_even_after_identical_recreation() {
    for clear in [false, true] {
        let store = MemoryIdentityStore::default();
        let key = store.key("revoked-fixture");
        let stamp = fixture_stamp(&store, &key, &["use"]);
        let pending = store.token_epoch(&key).unwrap();
        let completed = store.finish_verification(&key, &pending, &stamp).unwrap();
        if clear {
            store.clear();
        } else {
            store.invalidate_token("revoked-fixture");
        }
        assert!(store.finish_verification(&key, &pending, &stamp).is_none());
        let next = store.token_epoch(&key).unwrap();
        let recreated = store.finish_verification(&key, &next, &stamp).unwrap();
        assert!(store.finish_verification(&key, &pending, &stamp).is_none());
        assert!(!store.token_current(&key, &completed));
        assert!(store.token_current(&key, &recreated));
    }
}

#[test]
fn bounded_registry_retains_pending_owners_and_reclaims_only_unowned_markers() {
    let store = MemoryIdentityStore::new(1);
    let first = store.key("first-fixture");
    let second = store.key("second-fixture");
    let stamp = fixture_stamp(&store, &first, &["use"]);
    let pending = store.token_epoch(&first).unwrap();
    assert!(store.token_epoch(&second).is_none());
    let completed = store.finish_verification(&first, &pending, &stamp).unwrap();
    drop(completed);
    // A publisher admitted under the shared epoch still owns its capacity.
    assert_eq!(store.active_receipts(), 1);
    assert!(store.token_epoch(&second).is_none());
    drop(pending);
    assert_eq!(store.active_receipts(), 0);
    let fresh = store.token_epoch(&second).unwrap();
    assert!(store.token_current(&second, &fresh));
    assert_eq!(store.epochs.lock().unwrap().len(), 1);
    assert!(!store.epochs.lock().unwrap().contains_key(&first));
    assert!(MemoryIdentityStore::new(0).token_epoch(&first).is_none());
}

#[test]
fn replaced_live_old_epoch_never_prevents_reclamation_of_unowned_current_entry() {
    let store = MemoryIdentityStore::new(1);
    let key = store.key("fixture");
    let a = fixture_stamp(&store, &key, &["use"]);
    let b = fixture_stamp(&store, &key, &[]);
    let pending = store.token_epoch(&key).unwrap();
    let old = store.finish_verification(&key, &pending, &a).unwrap();
    let new = store.finish_verification(&key, &old, &b).unwrap();
    drop(new);
    assert_eq!(store.active_receipts(), 0);
    let fresh = store.token_epoch(&key).unwrap();
    assert!(!store.token_current(&key, &old));
    assert!(store.finish_verification(&key, &old, &a).is_none());
    assert!(store.finish_verification(&key, &fresh, &a).is_some());
}

#[test]
fn simultaneous_equal_publishers_both_keep_their_admitted_epoch() {
    let store = MemoryIdentityStore::default();
    let key = store.key("concurrent-fixture");
    let stamp = fixture_stamp(&store, &key, &["use"]);
    let pending = store.token_epoch(&key).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let completed = std::thread::scope(|scope| {
        let publish = || {
            barrier.wait();
            store.finish_verification(&key, &pending, &stamp).unwrap()
        };
        let a = scope.spawn(publish);
        let b = scope.spawn(publish);
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert!(completed
        .iter()
        .all(|receipt| Arc::ptr_eq(receipt, &pending)));
    assert_eq!(store.active_receipts(), 1);
    drop(pending);
    assert_eq!(store.active_receipts(), 1);
    drop(completed);
    assert_eq!(store.active_receipts(), 0);
}
