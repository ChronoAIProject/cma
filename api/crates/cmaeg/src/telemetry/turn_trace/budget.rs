//! Measure the synchronous producer and source-reader work, excluding sink IO.
use super::{testing, Operation, Outcome, ProviderReader, Stage};
use axum::http::HeaderMap;
use serde_json::json;
use std::time::{Duration, Instant};

fn percentile(samples: &mut [u128], percentile: usize) -> u128 {
    samples.sort_unstable();
    samples[(samples.len() - 1) * percentile / 100]
}

#[test]
fn turn_trace_producer_and_admission_reader_stay_within_synchronous_budgets() {
    let (writer, records, _guard) = testing::capture();
    let _installed = testing::install(writer.clone());
    let request = uuid::Uuid::new_v4();
    let turn = uuid::Uuid::new_v4();
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", request.to_string().parse().unwrap());
    headers.insert("x-nyxid-trace-id", request.to_string().parse().unwrap());
    let snapshot = json!({"id":"resp_budget", "nyxid_trace":{
        "version":1,"turn_id":turn.to_string(),"trace_id":turn.simple().to_string(),
        "parent_span_id":"0123456789abcdef","ingress_request_id":request.to_string()
    }});
    let mut boundary = Vec::with_capacity(2048);
    let mut admission = Vec::with_capacity(1024);
    for batch in 0..32 {
        for _ in 0..32 {
            let at = Instant::now();
            let operation = Operation::start(writer.clone(), Stage::Gateway, None, None);
            boundary.push(at.elapsed().as_nanos());
            let at = Instant::now();
            operation.finish(Outcome::Completed, None);
            boundary.push(at.elapsed().as_nanos());

            let at = Instant::now();
            let mut reader = ProviderReader::new(true, true, None).unwrap();
            reader.begin_source();
            reader.response(&headers, 200);
            reader.snapshot(&snapshot);
            assert_eq!(reader.association().unwrap().turn_id, turn);
            admission.push(at.elapsed().as_nanos());
        }
        // Keep the benchmark below the bounded queue's capacity and prove the
        // successful path actually serializes every record. Drain outside the
        // synchronous boundary measurement; blocked-output tests cover drops.
        for _ in 0..(32 * 6) {
            records.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        assert_eq!(writer.dropped(), 0, "records dropped in batch {batch}");
    }
    let boundary_p99_ns = percentile(&mut boundary, 99);
    let admission_p95_ns = percentile(&mut admission, 95);
    println!(
        "{}",
        json!({"boundary_p99_ns":boundary_p99_ns,
        "source_reader_p95_ns":admission_p95_ns,"boundaries":boundary.len(),
        "admissions":admission.len(),"dropped":writer.dropped()})
    );
    assert!(
        boundary_p99_ns < 100_000,
        "boundary p99 exceeded 100 microseconds"
    );
    assert!(
        admission_p95_ns < 1_000_000,
        "source reader p95 exceeded 1 millisecond"
    );
    assert!(records.try_recv().is_err());
}
