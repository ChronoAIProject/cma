use super::*;
use crate::allocation_probe::AllocationProbe;
use serde_json::json;

#[test]
fn maximal_invalid_discriminator_does_not_expand_into_an_owned_diagnostic() {
    let value = json!({"sessionUpdate":"x".repeat(4_193_900)});
    let encoded = serde_json::to_vec(&value).unwrap();
    let shape = crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    assert!(shape.nodes < 5);
    for bounded in [false, true] {
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| {
            decode::<crate::SessionUpdate>(&value, bounded)
                .err()
                .unwrap()
        });
        let (retained, peak) = (probe.live(), probe.peak());
        if bounded {
            assert_eq!(retained, 0, "bounded diagnostics own no input strings");
            assert!(
                peak < 64 * 1024,
                "only the small tag container may allocate"
            );
            assert!(error.to_string().len() < 128);
        } else {
            assert!(
                retained > 1024 * 1024,
                "fixture must expose the original diagnostic copy"
            );
        }
        drop(error);
        assert_eq!(probe.live(), 0);
        eprintln!("Typed diagnostic allocation bounded={bounded}: encoded={} nodes={} retained={retained} peak={peak} final=0", encoded.len(), shape.nodes);
    }
}

#[test]
fn discarded_optional_field_error_also_uses_the_bounded_diagnostic() {
    let value = json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"tool","locations":[{"path":"/workspace/file","line":"\0".repeat(698_000)}]});
    let encoded = serde_json::to_vec(&value).unwrap();
    crate::hosted::output_json_budget::inspect(&encoded).unwrap();
    let mut expected = None;
    for bounded in [false, true] {
        let mut probe = AllocationProbe::new();
        let update = probe.track(|| decode::<crate::SessionUpdate>(&value, bounded).unwrap());
        let (retained, peak) = (probe.live(), probe.peak());
        if bounded {
            assert!(
                peak < 64 * 1024,
                "a swallowed error must not copy the invalid field"
            );
        } else {
            assert!(
                peak > 1024 * 1024,
                "fixture must observe the discarded diagnostic"
            );
        }
        let result = serde_json::to_value(&update).unwrap();
        if let Some(expected) = &expected {
            assert_eq!(&result, expected);
        } else {
            expected = Some(result);
        }
        drop(update);
        assert_eq!(probe.live(), 0);
        eprintln!("Typed tolerant diagnostic allocation bounded={bounded}: encoded={} retained={retained} peak={peak} final=0", encoded.len());
    }
}
