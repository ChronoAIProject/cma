//! Optional optimized, Mongo-free diagnosis. Never substitutes for report timing.
use super::*;
use layer2_core::sandbox_usage::UsageEvent;
use serde_json::{json, Value};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

const MEMBERS: usize = 200;
const EVENTS_PER_MEMBER: usize = 288;
const REPEATS: usize = 5;

fn samples(mut operation: impl FnMut() -> u64) -> Value {
    let mut elapsed = Vec::new();
    let mut checksums = Vec::new();
    for _ in 0..REPEATS {
        let began = Instant::now();
        checksums.push(black_box(operation()));
        elapsed.push(began.elapsed().as_micros() as u64);
    }
    assert!(checksums.windows(2).all(|pair| pair[0] == pair[1]));
    let mut ordered = elapsed.clone();
    ordered.sort_unstable();
    json!({"samples_us":elapsed,"min_us":ordered[0],"median_us":ordered[REPEATS/2],
        "max_us":ordered[REPEATS-1],"checksum":checksums[0]})
}

#[test]
#[ignore = "optimized supporting CPU diagnosis; run only in the bound native campaign"]
fn native_usage_hotfix_decode_phase_calibration() {
    assert!(
        !black_box(cfg!(debug_assertions)),
        "use the production-like release profile"
    );
    let output = std::env::var("CHRONO_USAGE_CPU_OUTPUT").expect("private output path required");
    let source =
        std::env::var("CHRONO_USAGE_CPU_SOURCE").expect("actual compiled source SHA required");
    assert!(source.len() == 40 && source.bytes().all(|b| b.is_ascii_hexdigit()));
    let from = 1_790_035_200;
    let range = Range {
        from,
        to: from + 86_400,
    };
    let preparation = Instant::now();
    let mut states = Vec::new();
    let mut documents = Vec::new();
    let mut encoded = Vec::new();
    let mut grouped = BTreeMap::<String, Vec<UsageEvent>>::new();
    for id in 0..MEMBERS {
        let mut s = state(id, from);
        s.last_observed_at = range.to;
        s.source_seq = EVENTS_PER_MEMBER as u64;
        s.source_revision = s.source_seq;
        s.revision = s.source_seq;
        s.totals = s.allocation.usage(86_400).unwrap();
        let mut events = Vec::new();
        for n in 0..EVENTS_PER_MEMBER {
            let mut e = super::history_batch::event(&s, n as u64, from + n as u64 * 300);
            e.ended_at = e.started_at + 300;
            e.quantity = e.allocation.usage(300).unwrap();
            let d = crate::store::sandbox_usage::event_document(&e).unwrap();
            encoded.push(mongodb::bson::serialize_to_vec(&d).unwrap());
            documents.push(d);
            events.push(e);
        }
        grouped.insert(s.sandbox_id.to_string(), events);
        states.push(s);
    }
    let bytes: usize = encoded.iter().map(Vec::len).sum();
    assert!(documents.len() <= 100_000 && bytes <= 64 * 1024 * 1024);
    let preparation_us = preparation.elapsed().as_micros();
    let mut phases = BTreeMap::new();
    phases.insert(
        "bson_bytes_to_document",
        samples(|| {
            let mut sum = 0;
            for bytes in &encoded {
                let d: Document = mongodb::bson::deserialize_from_slice(black_box(bytes)).unwrap();
                sum += black_box(d.len() as u64);
            }
            sum
        }),
    );
    phases.insert(
        "document_to_bytes_for_exact_byte_cap",
        samples(|| {
            documents
                .iter()
                .map(|d| {
                    black_box(mongodb::bson::serialize_to_vec(black_box(d)).unwrap()).len() as u64
                })
                .sum()
        }),
    );
    phases.insert(
        "decode_record_only_without_query_field_validation",
        samples(|| {
            documents
                .iter()
                .map(|d| {
                    let e: UsageEvent =
                        crate::store::sandbox_mongo::read_record(black_box(d)).unwrap();
                    black_box(e).revision
                })
                .sum()
        }),
    );
    phases.insert(
        "actual_read_event_with_full_query_field_validation",
        samples(|| {
            documents
                .iter()
                .map(|d| {
                    black_box(crate::store::sandbox_usage::read_event(black_box(d)).unwrap())
                        .revision
                })
                .sum()
        }),
    );
    phases.insert(
        "actual_byte_cap_decode_validate_and_group",
        samples(|| {
            let mut events = BTreeMap::<String, Vec<UsageEvent>>::new();
            let mut bytes = 0usize;
            for (index, d) in documents.iter().enumerate() {
                bytes = bytes
                    .checked_add(mongodb::bson::serialize_to_vec(d).unwrap().len())
                    .unwrap();
                assert!(index < 100_000 && bytes <= 64 * 1024 * 1024);
                let e = crate::store::sandbox_usage::read_event(d).unwrap();
                events.entry(e.sandbox_id.to_string()).or_default().push(e);
            }
            let count = events.values().map(Vec::len).sum::<usize>() as u64;
            black_box(events);
            count
        }),
    );
    // Clone/preparation is outside these sort measurements. Both sorted and
    // reversed inputs use the candidate's actual u64 revision sort primitive.
    for reverse in [false, true] {
        let mut elapsed = Vec::new();
        for _ in 0..REPEATS {
            let mut events = grouped.clone();
            if reverse {
                for events in events.values_mut() {
                    events.reverse();
                }
            }
            let began = Instant::now();
            for events in events.values_mut() {
                events.sort_unstable_by_key(|e| e.revision);
            }
            black_box(&events);
            elapsed.push(began.elapsed().as_micros() as u64);
            assert!(events
                .values()
                .all(|v| v.windows(2).all(|w| w[0].revision < w[1].revision)));
        }
        let mut ordered = elapsed.clone();
        ordered.sort_unstable();
        phases.insert(if reverse {"revision_sort_reversed"} else {"revision_sort_sorted"},
            json!({"samples_us":elapsed,"median_us":ordered[REPEATS/2],"max_us":ordered[REPEATS-1]}));
    }
    phases.insert(
        "actual_allocation_projection",
        samples(|| {
            let mut uptime = 0;
            for s in &states {
                let p = allocation::project(s, &grouped[s.sandbox_id.as_str()], range);
                assert!(p.period.complete);
                assert_eq!(p.period.uptime_seconds, 86_400.);
                uptime += black_box(p).period.uptime_seconds as u64;
            }
            uptime
        }),
    );
    let record = json!({"source":source,"members":MEMBERS,"events":documents.len(),
        "bson_bytes":bytes,"repeats":REPEATS,"preparation_us":preparation_us,
        "debug_assertions":cfg!(debug_assertions),"phases":phases,
        "limits":["Supporting in-memory elapsed time, not CPU accounting or report latency",
            "Phase measurements overlap semantically and must not be added",
            "Decode-only skips security/consistency checks for diagnosis; never a replacement reader",
            "Production fixture uses varied retained scopes; this corpus is one full 200-member day",
            "No Mongo, async cursor, cache writes, service route or render is measured"]});
    std::fs::write(output, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}
