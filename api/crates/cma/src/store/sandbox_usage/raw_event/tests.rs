use super::*;
use crate::store::sandbox_usage::{event_document, read_event};
use layer2_core::{
    sandbox_usage::{Allocation, UsageEventKind},
    SandboxId, SubjectId,
};
use mongodb::bson::{doc, Bson, Document, RawDocumentBuf};

fn event(revision: u64) -> UsageEvent {
    let allocation = Allocation {
        running: true,
        cpu_millis: 2000,
        memory_mib: 1024,
        disk_gib: 64,
    };
    UsageEvent {
        sandbox_id: SandboxId("sbx_decoder_fixture".into()),
        owner: SubjectId("decoder-owner".into()),
        revision,
        source_seq: revision,
        kind: UsageEventKind::Checkpoint,
        started_at: 100,
        ended_at: 400,
        allocation: allocation.clone(),
        next_allocation: allocation.clone(),
        status: "active".into(),
        quantity: allocation.usage(300).unwrap(),
    }
}
fn raw(document: &Document) -> RawDocumentBuf {
    RawDocumentBuf::try_from(document).unwrap()
}
fn same(document: &Document) {
    let expected = read_event(document);
    let actual = read(&raw(document));
    assert_eq!(expected.is_ok(), actual.is_ok());
    if let Ok(expected) = expected {
        assert_eq!(actual.unwrap(), expected);
    }
}
#[test]
fn allocation_raw_event_preserves_versions_integer_precision_and_billing_metadata() {
    for revision in [0, 1, (1_u64 << 53) + 1, i64::MAX as u64] {
        for kind in [
            UsageEventKind::Created,
            UsageEventKind::Started,
            UsageEventKind::Paused,
            UsageEventKind::Resumed,
            UsageEventKind::Terminated,
            UsageEventKind::ResourcesChanged,
            UsageEventKind::StateChanged,
            UsageEventKind::Checkpoint,
            UsageEventKind::Baseline,
        ] {
            let mut event = event(revision);
            event.kind = kind;
            let mut document = event_document(&event).unwrap();
            for version in [Bson::Int32(1), Bson::Int64(1)] {
                document.insert("document_schema_version", version);
                same(&document);
                assert_eq!(read(&raw(&document)).unwrap(), event);
            }
            document.insert("billing_delivered", true);
            document.insert("billing_retry_at", 900_i64);
            document.insert("additional_metadata", doc! {"safe":true});
            same(&document);
        }
    }
}
#[test]
fn allocation_raw_event_refuses_missing_drifted_or_wrong_typed_envelopes() {
    let original = event_document(&event(1)).unwrap();
    for field in [
        "_id",
        "owner",
        "sandbox_id",
        "revision",
        "started_at",
        "ended_at",
        "record",
        "document_schema_version",
    ] {
        let mut missing = original.clone();
        missing.remove(field);
        same(&missing);
        assert!(read(&raw(&missing)).is_err());
        for value in [
            Bson::Null,
            Bson::String("different".into()),
            Bson::Int32(2),
            Bson::Int64(-1),
            Bson::Double(1.0),
        ] {
            let mut changed = original.clone();
            changed.insert(field, value);
            same(&changed);
            assert!(read(&raw(&changed)).is_err());
        }
    }
    for field in ["revision", "started_at", "ended_at"] {
        let mut small = original.clone();
        let value = small.get_i64(field).unwrap() as i32;
        small.insert(field, value);
        same(&small);
        assert!(read(&raw(&small)).is_err());
    }
}
#[test]
fn allocation_raw_event_keeps_nested_record_validation_strict() {
    let original = event_document(&event(1)).unwrap();
    for (field, value) in [
        ("extra", Bson::Boolean(true)),
        ("owner", "different".into()),
        ("revision", (-1_i64).into()),
        ("started_at", (-1_i64).into()),
        ("ended_at", (-1_i64).into()),
        ("kind", "unknown".into()),
        (
            "allocation",
            doc! {"running":true,"cpu_millis":2000,"memory_mib":1024,"disk_gib":64,"extra":1}
                .into(),
        ),
    ] {
        let mut changed = original.clone();
        changed
            .get_document_mut("record")
            .unwrap()
            .insert(field, value);
        same(&changed);
        assert!(read(&raw(&changed)).is_err());
    }
    let mut missing = original.clone();
    missing
        .get_document_mut("record")
        .unwrap()
        .remove("quantity");
    same(&missing);
    assert!(read(&raw(&missing)).is_err());
}
#[test]
fn allocation_raw_event_rejects_duplicate_authority_and_truncated_bytes() {
    let original = event_document(&event(1)).unwrap();
    let encoded = raw(&original);
    for field in [
        "_id",
        "owner",
        "sandbox_id",
        "revision",
        "started_at",
        "ended_at",
        "record",
        "document_schema_version",
    ] {
        let mut duplicate = encoded.clone();
        let key = mongodb::bson::raw::CString::try_from(field).unwrap();
        duplicate.append(key, encoded.get(field).unwrap().unwrap());
        assert!(read(&duplicate).is_err());
    }
    for length in [0, 1, 4, encoded.as_bytes().len() - 1] {
        assert!(RawDocument::from_bytes(&encoded.as_bytes()[..length]).is_err());
    }
}

#[test]
#[ignore = "bounded CPU comparison; explicit invocation records timings, no database or report-latency claim"]
fn allocation_raw_event_cpu_comparison() {
    use std::{hint::black_box, time::Instant};
    let documents = (1..=2_000)
        .map(|revision| raw(&event_document(&event(revision)).unwrap()))
        .collect::<Vec<_>>();
    let samples = 3;
    let repeats = 3;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..samples {
        let began = Instant::now();
        let mut checksum = 0;
        let mut bytes = 0;
        for _ in 0..repeats {
            for raw in &documents {
                let document: Document =
                    bson::deserialize_from_slice(black_box(raw.as_bytes())).unwrap();
                bytes += bson::serialize_to_vec(&document).unwrap().len();
                checksum += black_box(read_event(&document).unwrap()).revision;
            }
        }
        before.push(began.elapsed().as_micros());
        let expected = (checksum, bytes);
        let began = Instant::now();
        let mut checksum = 0;
        let mut bytes = 0;
        for _ in 0..repeats {
            for raw in &documents {
                bytes += raw.as_bytes().len();
                checksum += black_box(read(black_box(raw)).unwrap()).revision;
            }
        }
        after.push(began.elapsed().as_micros());
        assert_eq!((checksum, bytes), expected);
    }
    println!(
        "ALLOCATION_DECODE_CPU {}",
        serde_json::json!({"events_per_sample":documents.len()*repeats,
        "samples":samples,"baseline_us":before,"candidate_us":after,"same_checksum_and_byte_count":true,
        "debug_assertions":cfg!(debug_assertions),"profile_source":"bound compiler artifact receipt","database":false,"full_report_measurement":false})
    );
}
