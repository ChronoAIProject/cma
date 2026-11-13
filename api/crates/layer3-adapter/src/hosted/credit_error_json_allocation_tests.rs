use crate::allocation_probe::AllocationProbe;
use crate::hosted::acp::Outcome;
use crate::hosted::acp_turn::turn_outcome_with_policy;

#[test]
fn admitted_error_string_cannot_materialize_an_unreserved_embedded_tree() {
    let elements = 2 * 1024 * 1024 - 256;
    let diagnostic = format!(
        "unexpected status 402 Payment Required: {{\"error\":\"insufficient_credits\",\"error_code\":11300,\"details\":[{}0]}}",
        "0,".repeat(elements - 1)
    );
    let frame = serde_json::json!({"error":{"message":diagnostic}});
    let encoded = serde_json::to_vec(&frame).unwrap();
    let shape = crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    assert!(encoded.len() <= 4 * 1024 * 1024);
    assert!(
        shape.nodes < 10,
        "the nested container is only an outer string"
    );
    let mut peaks = Vec::new();
    for bounded in [false, true] {
        let mut probe = AllocationProbe::new();
        let outcome = probe.track(|| turn_outcome_with_policy(&frame, bounded));
        assert!(matches!(&outcome, Outcome::Failed {capability, ..}
            if capability.as_deref() == Some("insufficient_credits")));
        let (retained, peak, allocated) = (probe.live(), probe.peak(), probe.allocated());
        drop(outcome);
        assert_eq!(
            probe.live(),
            0,
            "final outcome owner must release every measured byte"
        );
        eprintln!("Embedded JSON allocation bounded={bounded}: encoded={} outer_nodes={} retained={retained} peak={peak} total_requested={allocated}", encoded.len(), shape.nodes);
        peaks.push(peak);
    }
    assert!(
        peaks[0] > 128 * 1024 * 1024,
        "fixture must expose the original unaccounted tree"
    );
    // This fixture needs only the existing diagnostic String copy; its unknown
    // container consists of unescaped primitive values and allocates no tree.
    assert!(peaks[1] <= encoded.len() + 64 * 1024);
}

#[test]
fn escaped_unknown_keys_use_bounded_parser_scratch_and_release_it() {
    let diagnostic = format!(
        "unexpected status 402 Payment Required: {{\"error\":\"insufficient_credits\",\"error_code\":11300,\"{}\":null}}",
        "\\n".repeat(512 * 1024)
    );
    let frame = serde_json::json!({"error":{"message":diagnostic}});
    let encoded = serde_json::to_vec(&frame).unwrap();
    crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    let mut probe = AllocationProbe::new();
    let outcome = probe.track(|| turn_outcome_with_policy(&frame, true));
    assert!(matches!(&outcome, Outcome::Failed {capability, ..}
        if capability.as_deref() == Some("insufficient_credits")));
    let (retained, peak, allocated) = (probe.live(), probe.peak(), probe.allocated());
    assert!(peak <= 2 * encoded.len() + 64 * 1024);
    drop(outcome);
    assert_eq!(probe.live(), 0);
    eprintln!("Embedded JSON escaped-key allocation: encoded={} retained={retained} peak={peak} total_requested={allocated}", encoded.len());
}
