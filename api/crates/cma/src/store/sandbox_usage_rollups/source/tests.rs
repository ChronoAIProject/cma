use super::*;
use layer2_core::sandbox_usage::{Allocation, SandboxUsage, UsageEventKind, UsageTotals};
use layer2_core::{SandboxId, SubjectId};

fn state(id: &str) -> SandboxUsage {
    SandboxUsage {
        sandbox_id: SandboxId(id.into()),
        owner: SubjectId("owner".into()),
        name: id.into(),
        status: "active".into(),
        allocation: Allocation {
            running: true,
            cpu_millis: 2000,
            memory_mib: 1024,
            disk_gib: 8,
        },
        created_at: 100,
        tracking_started_at: 100,
        last_observed_at: 340,
        source_seq: 4,
        source_revision: 4,
        revision: 4,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    }
}

fn event(state: &SandboxUsage, revision: u64) -> UsageEvent {
    UsageEvent {
        sandbox_id: state.sandbox_id.clone(),
        owner: state.owner.clone(),
        revision,
        source_seq: revision,
        kind: UsageEventKind::Checkpoint,
        started_at: 100 + (revision - 1) * 60,
        ended_at: 100 + revision * 60,
        allocation: state.allocation.clone(),
        next_allocation: state.allocation.clone(),
        status: state.status.clone(),
        quantity: state.allocation.usage(60).unwrap(),
    }
}

fn document(event: &UsageEvent) -> mongodb::bson::Document {
    crate::store::sandbox_usage::event_document(event).unwrap()
}

#[test]
fn allocation_interval_restores_each_sandbox_revision_order_and_exact_projection() {
    let states = [state("b"), state("a")];
    let mut batch = EventBatch::default();
    for revision in [4, 2, 3, 1] {
        for state in &states {
            batch.push(&raw(document(&event(state, revision)))).unwrap();
        }
    }
    let result = batch.finish(false);
    assert_eq!(
        result.keys().map(String::as_str).collect::<Vec<_>>(),
        ["a", "b"]
    );
    for state in &states {
        let expected = (1..=4)
            .map(|revision| event(state, revision))
            .collect::<Vec<_>>();
        let actual = &result[state.sandbox_id.as_str()];
        assert_eq!(actual, &expected);
        let range = Range { from: 130, to: 330 };
        let projected = allocation::project(state, actual, range);
        assert!(projected.period.complete);
        assert_eq!(
            serde_json::to_value(projected).unwrap(),
            serde_json::to_value(allocation::project(state, &expected, range)).unwrap()
        );
    }
    assert!(EventBatch::default().finish(false).is_empty());
}

#[test]
fn allocation_interval_orders_large_revisions_without_precision_loss() {
    let state = state("a");
    let mut batch = EventBatch::default();
    let start = i64::MAX as u64 - 3;
    for revision in [start + 2, start, start + 1] {
        let mut e = event(&state, 1);
        e.revision = revision;
        batch.push(&raw(document(&e))).unwrap();
    }
    assert_eq!(
        batch.finish(false)["a"]
            .iter()
            .map(|e| e.revision)
            .collect::<Vec<_>>(),
        [start, start + 1, start + 2]
    );
}

#[test]
fn allocation_interval_keeps_gaps_duplicates_and_foreign_owners_incomplete() {
    let state = state("a");
    for revisions in [vec![3, 1, 4], vec![4, 2, 2, 1, 3]] {
        let mut batch = EventBatch::default();
        for revision in revisions {
            batch
                .push(&raw(document(&event(&state, revision))))
                .unwrap();
        }
        let events = batch.finish(false);
        assert!(
            !allocation::project(&state, &events["a"], Range { from: 100, to: 340 })
                .period
                .complete
        );
    }
    let mut foreign = event(&state, 1);
    foreign.owner = SubjectId("foreign".into());
    let mut batch = EventBatch::default();
    batch.push(&raw(document(&foreign))).unwrap();
    assert!(
        !allocation::project(
            &state,
            &batch.finish(false)["a"],
            Range { from: 100, to: 160 }
        )
        .period
        .complete
    );
}

#[test]
fn allocation_interval_rejects_malformed_records_and_mismatched_query_fields() {
    let original = document(&event(&state("a"), 1));
    for key in [
        "_id",
        "owner",
        "sandbox_id",
        "revision",
        "started_at",
        "ended_at",
        "record",
        "document_schema_version",
    ] {
        let mut bad = original.clone();
        bad.insert(key, "corrupt");
        let mut batch = EventBatch::default();
        batch.push(&raw(original.clone())).unwrap();
        assert!(
            matches!(batch.push(&raw(bad)), Err(MetricsError::Unavailable)),
            "{key}"
        );
    }
}

#[test]
fn allocation_interval_keeps_exact_event_and_byte_caps() {
    let document = document(&event(&state("a"), 1));
    let length = mongodb::bson::serialize_to_vec(&document).unwrap().len();
    let mut count = EventBatch {
        count: 99_999,
        ..Default::default()
    };
    count.push(&raw(document.clone())).unwrap();
    assert!(matches!(
        count.push(&raw(document.clone())),
        Err(MetricsError::Busy)
    ));
    let mut bytes = EventBatch {
        bytes: 64 * 1024 * 1024 - length,
        ..Default::default()
    };
    bytes.push(&raw(document.clone())).unwrap();
    assert!(matches!(
        bytes.push(&raw(document.clone())),
        Err(MetricsError::Busy)
    ));
    let mut overflow = EventBatch {
        bytes: usize::MAX,
        ..Default::default()
    };
    assert!(matches!(
        overflow.push(&raw(document)),
        Err(MetricsError::Busy)
    ));
}

fn raw(document: mongodb::bson::Document) -> mongodb::bson::RawDocumentBuf {
    mongodb::bson::RawDocumentBuf::try_from(document).unwrap()
}
