use super::*;
use crate::console::test_support::{authorized, record};

#[test]
fn heads_are_owner_scoped_and_follow_their_latest_version() {
    let index = OwnerIndex::new(1_000);
    index.upsert(record(1), "alice".into(), 1);
    index.upsert(record(2), "bob".into(), 1);
    index.upsert(record(1), "alice".into(), 3);
    // A page listed before the write arrives after it.
    index.upsert(record(1), "alice".into(), 2);
    let alice = authorized("alice");
    let bob = authorized("bob");
    assert_eq!(index.version_for(&alice, &record(1)), Some(3));
    assert_eq!(index.version_for(&bob, &record(1)), None);
    assert_eq!(index.records_for(&alice), [(record(1), 3)]);
    assert_eq!(index.records_for(&bob), [(record(2), 1)]);
    assert!(index.records_for(&authorized("carol")).is_empty());
    index.remove(&record(1));
    assert_eq!(index.version_for(&alice, &record(1)), None);
    assert!(index.records_for(&alice).is_empty());
    assert_eq!(index.status().records, 1);
}

#[test]
fn a_changed_owner_follows_cma() {
    let index = OwnerIndex::new(1_000);
    index.upsert(record(1), "alice".into(), 1);
    index.upsert(record(1), "bob".into(), 1);
    assert_eq!(index.version_for(&authorized("alice"), &record(1)), None);
    assert_eq!(index.version_for(&authorized("bob"), &record(1)), Some(1));
    assert!(index.records_for(&authorized("alice")).is_empty());
}

#[test]
fn beyond_its_bound_the_index_is_partial() {
    let index = OwnerIndex::new(2);
    for position in 0..3 {
        index.upsert(record(position), "alice".into(), 1);
    }
    let status = index.status();
    assert_eq!(status.records, 2);
    assert!(status.partial);
    // Known records still update while the index is full.
    index.upsert(record(0), "alice".into(), 5);
    assert_eq!(index.version_for(&authorized("alice"), &record(0)), Some(5));
    assert_eq!(index.version_for(&authorized("alice"), &record(2)), None);
}

#[test]
fn a_complete_listing_retires_unseen_records_but_not_newer_writes() {
    let index = OwnerIndex::new(1_000);
    index.upsert(record(1), "alice".into(), 1);
    index.upsert(record(2), "alice".into(), 1);
    let since = index.begin(true);
    assert_eq!(index.status().loading, Some(0));
    // The listing sees record 1; record 3 is created while it runs.
    index.upsert(record(1), "alice".into(), 1);
    index.progress(1);
    assert_eq!(index.status().loading, Some(1));
    index.upsert(record(3), "alice".into(), 1);
    index.retire(since, None);
    let alice = authorized("alice");
    assert_eq!(index.records_for(&alice), [(record(1), 1), (record(3), 1)]);
    assert_eq!(index.status().loading, None);
}

#[test]
fn an_owner_listing_retires_only_that_owners_records() {
    let index = OwnerIndex::new(1_000);
    index.upsert(record(1), "alice".into(), 1);
    index.upsert(record(2), "bob".into(), 1);
    let since = index.begin(false);
    index.retire(since, Some(&authorized("alice")));
    assert!(index.records_for(&authorized("alice")).is_empty());
    assert_eq!(index.records_for(&authorized("bob")), [(record(2), 1)]);
}

#[test]
fn an_abandoned_listing_removes_nothing() {
    let index = OwnerIndex::new(1_000);
    index.upsert(record(1), "alice".into(), 1);
    index.begin(true);
    index.abandon();
    assert_eq!(index.status().loading, None);
    assert_eq!(index.status().records, 1);
}

#[test]
fn listed_heads_with_invalid_ids_are_skipped() {
    let valid = RecordHead {
        record_id: record(1).to_string(),
        owner: "alice".into(),
        version: 4,
    };
    assert_eq!(entry(&valid), Some((record(1), "alice".to_owned(), 4)));
    let invalid = RecordHead {
        record_id: String::new(),
        owner: "alice".into(),
        version: 1,
    };
    assert_eq!(entry(&invalid), None);
}
