//! Owner checks on every in-memory read (Amendment R6 §R6.4). These run
//! inside the crate because only the console can construct `Authorized`.
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use super::*;
use crate::{
    console::{
        cma_budget::Pacing,
        memory::{Schedule, Sources},
        test_support::{
            authorized, channel_id, definition, record, FakeHeads, FakeReader, ReadFailure,
        },
    },
    ConsoleMemoryConfig,
};

/// Memory over `reader`, with alice owning records 1 and 2 and bob record 3.
fn memory(reader: Arc<FakeReader>) -> ConsoleMemory {
    for (index, owner, name) in [
        (1, "alice", "orders"),
        (2, "alice", "support"),
        (3, "bob", "billing"),
    ] {
        reader.store(definition(&record(index), owner, name), 1);
    }
    let heads = Arc::new(FakeHeads::default());
    for (index, owner) in [(1, "alice"), (2, "alice"), (3, "bob")] {
        heads
            .heads
            .lock()
            .unwrap()
            .push(crate::records::RecordHead {
                record_id: record(index).to_string(),
                owner: owner.into(),
                version: 1,
            });
    }
    let memory = ConsoleMemory::new(
        &ConsoleMemoryConfig::default(),
        Sources { heads, reader },
        Schedule {
            settle: Duration::ZERO,
            resync: Duration::from_secs(3_600),
            pacing: Pacing::default(),
        },
    );
    for (index, owner) in [(1, "alice"), (2, "alice"), (3, "bob")] {
        memory.index.upsert(record(index), owner.into(), 1);
    }
    memory
}

#[tokio::test]
async fn another_owners_records_read_as_none_everywhere() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader.clone());
    let alice = authorized("alice");
    let mallory = authorized("mallory");
    // Alice's summary is cached, so only the owner checks stand between it
    // and another reader.
    assert!(memory.summary(&alice, &record(1)).await.is_some());
    assert!(memory.summary(&mallory, &record(1)).await.is_none());
    assert!(memory.channel_id(&alice, &record(1)).is_some());
    assert!(memory.channel_id(&mallory, &record(1)).is_none());
    assert_eq!(
        memory
            .link(&mallory, &record(1), &channel_id(&record(1)))
            .await,
        Err(LinkError::NotFound)
    );
    let page = memory.channels_for(&mallory, 0, None).await;
    assert!(page.rows.is_empty());
    assert_eq!(page.total, 0);
    // Bob's record is not Alice's, whatever she asks.
    assert!(memory.summary(&alice, &record(3)).await.is_none());
    assert!(memory.channel_id(&alice, &record(3)).is_none());
}

#[tokio::test]
async fn a_summary_never_crosses_owners_even_if_cached() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader);
    // A summary cached under alice's head but stored with another owner is
    // refused by its own owner check.
    memory.summaries.insert(
        record(1),
        1,
        Arc::new(ChannelSummary::without_definition(
            "bob",
            SummaryStatus::Ready,
        )),
    );
    assert!(memory
        .summary(&authorized("alice"), &record(1))
        .await
        .is_none());
}

#[tokio::test]
async fn page_renders_reuse_cached_summaries_until_the_version_changes() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader.clone());
    let alice = authorized("alice");
    let page = memory.channels_for(&alice, 0, None).await;
    let names: Vec<_> = page.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["orders", "support"]);
    assert_eq!(page.total, 2);
    let reads = reader.reads.load(Ordering::SeqCst);
    assert_eq!(reads, 2);
    memory.channels_for(&alice, 0, None).await;
    assert_eq!(
        reader.reads.load(Ordering::SeqCst),
        reads,
        "a refresh read CMA"
    );
    // A new version is read once.
    reader.store(definition(&record(1), "alice", "orders-v2"), 2);
    memory.index.upsert(record(1), "alice".into(), 2);
    let summary = memory.summary(&alice, &record(1)).await.unwrap();
    assert_eq!(summary.name, "orders-v2");
    assert_eq!(reader.reads.load(Ordering::SeqCst), reads + 1);
}

#[tokio::test]
async fn pages_filter_by_name_and_page_fifty_at_a_time() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader.clone());
    let alice = authorized("alice");
    let page = memory.channels_for(&alice, 0, Some("SUP")).await;
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0].name, "support");
    for index in 10..70 {
        reader.store(
            definition(&record(index), "alice", &format!("bulk-{index}")),
            1,
        );
        memory.index.upsert(record(index), "alice".into(), 1);
    }
    let first = memory.channels_for(&alice, 0, None).await;
    assert_eq!(first.total, 62);
    assert_eq!(first.rows.len(), PAGE_ROWS);
    let second = memory.channels_for(&alice, 1, None).await;
    assert_eq!(second.rows.len(), 12);
}

#[tokio::test]
async fn a_record_without_a_head_is_read_once_and_indexed_for_its_owner() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader.clone());
    reader.store(definition(&record(7), "alice", "late"), 4);
    let alice = authorized("alice");
    let summary = memory.summary(&alice, &record(7)).await.unwrap();
    assert_eq!(summary.name, "late");
    assert_eq!(memory.index.version_for(&alice, &record(7)), Some(4));
    // Another owner's unindexed record is not theirs, and a miss is not
    // read again at once.
    reader.store(definition(&record(8), "bob", "hidden"), 1);
    let reads = reader.reads.load(Ordering::SeqCst);
    assert!(memory.summary(&alice, &record(8)).await.is_none());
    assert!(memory.summary(&alice, &record(8)).await.is_none());
    assert_eq!(reader.reads.load(Ordering::SeqCst), reads + 1);
    assert_eq!(
        memory.index.version_for(&authorized("bob"), &record(8)),
        None
    );
}

#[tokio::test]
async fn unreadable_and_unavailable_records_are_named_by_status_only() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader.clone());
    let alice = authorized("alice");
    *reader.failure.lock().unwrap() = Some(ReadFailure::Unavailable);
    let summary = memory.summary(&alice, &record(1)).await.unwrap();
    assert_eq!(summary.status, SummaryStatus::Unavailable);
    // Unavailable is never cached.
    *reader.failure.lock().unwrap() = Some(ReadFailure::Unreadable);
    let summary = memory.summary(&alice, &record(1)).await.unwrap();
    assert_eq!(summary.status, SummaryStatus::Unreadable);
    assert!(summary.name.is_empty());
    *reader.failure.lock().unwrap() = None;
    let cached = memory.summary(&alice, &record(1)).await.unwrap();
    assert_eq!(cached.status, SummaryStatus::Unreadable);
    // Neither reveals another owner's record.
    assert!(memory
        .summary(&authorized("bob"), &record(1))
        .await
        .is_none());
}

#[tokio::test]
async fn channel_ids_come_from_creation_traffic_and_checked_paste_links() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader);
    let alice = authorized("alice");
    assert_eq!(memory.channel_id(&alice, &record(2)), None);
    memory.summary(&alice, &record(2)).await.unwrap();
    assert_eq!(
        memory.channel_id(&alice, &record(2)),
        Some(channel_id(&record(2)))
    );
    memory.apply(crate::console::observer::Observation::ChannelId {
        record: record(1),
        id: channel_id(&record(1)),
    });
    assert_eq!(
        memory.channel_id(&alice, &record(1)),
        Some(channel_id(&record(1)))
    );
    let foreign = ChannelId::new("cmaeg1.ch.other").unwrap();
    assert_eq!(
        memory.link(&alice, &record(1), &foreign).await,
        Err(LinkError::Malformed)
    );
    assert_eq!(
        memory
            .link(&alice, &record(1), &channel_id(&record(2)))
            .await,
        Err(LinkError::Mismatch)
    );
    assert_eq!(
        memory
            .link(&alice, &record(1), &channel_id(&record(1)))
            .await,
        Ok(())
    );
}

#[tokio::test]
async fn summaries_hold_no_profile_or_secret() {
    let reader = Arc::new(FakeReader::default());
    let memory = memory(reader);
    let summary = memory
        .summary(&authorized("alice"), &record(1))
        .await
        .unwrap();
    let dump = serde_json::to_string(&*summary).unwrap();
    for secret in ["fixture-channel-key", "system_prompt", "gpt-test"] {
        assert!(!dump.contains(secret), "{secret} in {dump}");
    }
    assert!(dump.contains("sha256:…"), "{dump}");
}
