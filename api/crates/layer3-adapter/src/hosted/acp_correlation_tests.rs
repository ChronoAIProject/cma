use super::*;
use crate::allocation_probe::AllocationProbe;
use serde_json::json;

#[test]
fn quoted_utf8_and_escape_boundaries_keep_the_existing_encoded_limit() {
    for (call, expected) in [
        (json!(""), true),
        (json!("x".repeat(254)), true),
        (json!("x".repeat(255)), false),
        (json!("é".repeat(127)), true),
        (json!("é".repeat(128)), false),
        (json!("\n".repeat(127)), true),
        (json!("\n".repeat(128)), false),
        (json!("\0".repeat(42)), true),
        (json!("\0".repeat(43)), false),
        (json!("\"".repeat(127)), true),
        (json!("\\".repeat(128)), false),
        (json!(i64::MIN), true),
        (json!(u64::MAX), true),
        (json!(-1.25e100), true),
        (Value::Null, false),
        (json!(true), false),
        (json!(["id"]), false),
        (json!({"id":"nested"}), false),
    ] {
        for bounded in [false, true] {
            assert_eq!(valid_call_with_policy(&call, bounded), expected, "{call:?}");
            assert_eq!(
                permission_call(&json!({"id":call}), bounded),
                expected.then(|| call.clone())
            );
        }
    }
    assert_eq!(permission_call(&json!({}), true), None);
    assert_eq!(permission_call(&json!({}), false), None);
}

#[test]
fn mixed_escape_serialization_matches_legacy_at_every_near_boundary() {
    for prefix in [
        "x", "é", "😀", "\0", "\n", "\t", "\"", "\\", "\u{1f}", "\u{2028}",
    ] {
        for count in 0..=260 {
            let call = json!(format!("{}\"é\\", prefix.repeat(count)));
            assert_eq!(valid_call_with_policy(&call, true), valid_call(&call));
        }
    }
}

fn maximal(kind: &str) -> Value {
    let mut frame = json!({"id":"","method":"session/request_permission","params":{"sessionId":"s","toolCall":{"toolCallId":"tool","title":"Permission"},"options":[]}});
    if kind == "container" {
        frame["id"] = json!([""]);
    }
    let remaining = 4 * 1024 * 1024 - serde_json::to_vec(&frame).unwrap().len();
    if kind == "container" {
        frame["id"][0] = json!("x".repeat(remaining));
    } else if kind == "escaped" {
        frame["id"] = json!(format!(
            "{}{}",
            "\0".repeat(remaining / 6),
            "x".repeat(remaining % 6)
        ));
    } else {
        frame["id"] = json!("x".repeat(remaining));
    }
    let encoded = serde_json::to_vec(&frame).unwrap();
    assert_eq!(encoded.len(), 4 * 1024 * 1024);
    crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    frame
}

#[test]
fn rejected_maximal_ids_do_not_allocate_full_clones_or_encodings() {
    for kind in ["text", "escaped", "container"] {
        let frame = maximal(kind);
        for bounded in [false, true] {
            let mut probe = AllocationProbe::new();
            let selected = probe.track(|| permission_call(&frame, bounded));
            assert!(selected.is_none());
            let peak = probe.peak();
            if bounded {
                assert!(peak < 1024, "fixed-buffer rejection peak {peak}");
            } else {
                assert!(peak >= 1024 * 1024);
            }
            assert_eq!(probe.live(), 0);
            eprintln!("Correlation rejection {kind} bounded={bounded}: encoded=4194304 peak={peak} final=0");
        }
    }
}

#[test]
fn valid_encoded_boundary_copies_only_the_accepted_id() {
    let frame = json!({"id":"x".repeat(254)});
    let mut probe = AllocationProbe::new();
    let selected = probe.track(|| permission_call(&frame, true).unwrap());
    assert_eq!(probe.peak(), 254);
    assert_eq!(probe.live(), 254);
    drop(selected);
    assert_eq!(probe.live(), 0);
}
