use super::*;
use serde_json::json;

fn association() -> Value {
    json!({"version":1,"turn_id":"11111111-1111-4111-8111-111111111111",
        "trace_id":"11111111111141118111111111111111","parent_span_id":"0123456789abcdef",
        "ingress_request_id":"22222222-2222-4222-8222-222222222222"})
}

fn reader(id: Option<&str>) -> ProviderReader {
    ProviderReader::new(true, true, id).unwrap()
}

#[test]
fn only_an_enabled_negotiated_reader_observes_an_association() {
    for (enabled, negotiated) in [(false, false), (false, true), (true, false)] {
        assert!(ProviderReader::new(enabled, negotiated, Some("resp_synthetic")).is_none());
    }
    for invalid in ["".to_owned(), "x".repeat(513), "resp\nprivate".to_owned()] {
        assert!(ProviderReader::new(true, true, Some(&invalid)).is_none());
    }
}

#[test]
fn queued_turn_binds_from_ordered_event_and_identical_recovery_snapshot() {
    let mut live = reader(None);
    assert_eq!(
        live.event(
            &json!({"type":"response.created","response":{"id":"resp_synthetic","status":"queued"}})
        ),
        Observation::Missing
    );
    assert!(live.turn_id().is_none());
    let event = json!({"type":"response.nyxid_trace","response_id":"resp_synthetic","nyxid_trace":association()});
    assert_eq!(live.event(&event), Observation::Bound);
    assert_eq!(live.event(&event), Observation::Duplicate);
    let snapshot = json!({"id":"resp_synthetic","status":"completed","nyxid_trace":association()});
    assert_eq!(live.snapshot(&snapshot), Observation::Duplicate);
    let mut recovered = reader(Some("resp_synthetic"));
    assert_eq!(recovered.snapshot(&snapshot), Observation::Bound);
    assert_eq!(recovered.event(&event), Observation::Duplicate);
    assert_eq!(live.association(), recovered.association());
}

#[test]
fn missing_metadata_from_old_peer_is_a_gap_for_every_lifecycle() {
    let mut reader = reader(None);
    for kind in [
        "created",
        "in_progress",
        "failed",
        "completed",
        "incomplete",
    ] {
        assert_eq!(
            reader.event(
                &json!({"type":format!("response.{kind}"),"response":{"id":"resp_synthetic"}})
            ),
            Observation::Missing
        );
    }
    assert!(reader.turn_id().is_none());
    assert!(reader.association().is_none());
}

#[test]
fn mismatched_response_and_conflicting_replays_never_replace_the_winner() {
    let mut reader = reader(Some("resp_synthetic"));
    let mut event = json!({"type":"response.nyxid_trace","response_id":"resp_other","nyxid_trace":association()});
    assert_eq!(reader.event(&event), Observation::Conflict);
    assert!(reader.association().is_none());
    event["response_id"] = json!("resp_synthetic");
    assert_eq!(reader.event(&event), Observation::Bound);
    let winner = reader.association();
    for field in ["parent_span_id", "ingress_request_id", "turn_id"] {
        let mut conflicting = event.clone();
        match field {
            "parent_span_id" => conflicting["nyxid_trace"][field] = json!("fedcba9876543210"),
            "ingress_request_id" => conflicting["nyxid_trace"][field] = json!(Uuid::new_v4()),
            _ => {
                let other = Uuid::new_v4();
                conflicting["nyxid_trace"][field] = json!(other);
                conflicting["nyxid_trace"]["trace_id"] = json!(other.simple().to_string());
            }
        }
        assert_eq!(reader.event(&conflicting), Observation::Conflict);
        assert_eq!(reader.association(), winner);
    }
    let mut malformed = event;
    malformed["nyxid_trace"]["version"] = json!(99);
    assert_eq!(reader.event(&malformed), Observation::Invalid);
    assert_eq!(reader.association(), winner);
}

#[test]
fn response_headers_distinguish_request_fallback_from_committed_turn() {
    let request = "22222222-2222-4222-8222-222222222222";
    let turn = "11111111-1111-4111-8111-111111111111";
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", request.parse().unwrap());
    headers.insert("x-nyxid-trace-id", request.parse().unwrap());
    let mut reader = reader(Some("resp_synthetic"));
    assert_eq!(reader.headers(&headers), Observation::Missing);
    assert_eq!(reader.request_id, uuid_v4(request));
    assert!(reader.turn_id().is_none());
    headers.insert("x-nyxid-trace-id", turn.parse().unwrap());
    assert_eq!(reader.headers(&headers), Observation::Bound);
    assert_eq!(reader.headers(&headers), Observation::Duplicate);
    assert_eq!(reader.turn_id(), uuid_v4(turn));
    assert_eq!(
        reader.association(),
        Some(Association {
            turn_id: uuid_v4(turn).unwrap(),
            parent_span_id: None,
            ingress_request_id: uuid_v4(request).unwrap(),
        })
    );
    assert_eq!(
        reader.snapshot(&json!({"id":"resp_synthetic","nyxid_trace":association()})),
        Observation::Bound
    );
    headers.insert(
        "x-nyxid-trace-id",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    assert_eq!(reader.headers(&headers), Observation::Conflict);
    assert_eq!(reader.turn_id(), uuid_v4(turn));
    assert_eq!(reader.association(), Association::parse(&association()));
}

#[test]
fn duplicate_or_private_header_values_cannot_establish_a_turn() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-request-id",
        "22222222-2222-4222-8222-222222222222".parse().unwrap(),
    );
    headers.insert(
        "x-nyxid-trace-id",
        "11111111-1111-4111-8111-111111111111".parse().unwrap(),
    );
    headers.append(
        "x-nyxid-trace-id",
        "33333333-3333-4333-8333-333333333333".parse().unwrap(),
    );
    let mut reader = reader(None);
    assert_eq!(reader.headers(&headers), Observation::Missing);
    assert!(reader.turn_id().is_none());
    headers.insert("x-nyxid-trace-id", "private-canary".parse().unwrap());
    assert_eq!(reader.headers(&headers), Observation::Missing);
    assert!(reader.turn_id().is_none());
}
