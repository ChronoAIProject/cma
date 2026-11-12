use super::*;

mod shape;

#[test]
fn capacity_recovery_carrier_roundtrips_only_the_versioned_bounded_binding() {
    let original = test_support::stored();
    let row = encode(&original).unwrap();
    assert_eq!(row.get_i64("format_version").unwrap(), 2);
    assert_eq!(decode(row.clone()).unwrap(), original);
    assert!(bson::serialize_to_vec(&row).unwrap().len() < 4096);
    // Hashes permit a later-head receipt read without copying the original input.
    let result = row.get_document("result").unwrap();
    assert_eq!(result.len(), 5);
    assert_eq!(result.get_binary_generic("agent_digest").unwrap().len(), 32);
    assert_eq!(result.get_binary_generic("logs_digest").unwrap().len(), 32);
}

#[test]
fn capacity_recovery_carrier_refuses_old_drafts_unknown_fields_and_oversized_rows() {
    let row = encode(&test_support::stored()).unwrap();
    for variant in 0..7 {
        let mut changed = row.clone();
        match variant {
            0 => {
                changed.insert("format_version", 1_i64);
            }
            1 => {
                changed.remove("result");
            }
            2 => {
                changed.insert("authority", true);
            }
            3 => {
                changed.insert("agent_id", "other");
            }
            4 => {
                changed.insert("padding", "x".repeat(32 * 1024));
            }
            5 => {
                changed
                    .get_document_mut("result")
                    .unwrap()
                    .insert("unexpected", true);
            }
            _ => {
                changed
                    .get_document_mut("result")
                    .unwrap()
                    .insert("through_seq", -1_i64);
            }
        }
        assert!(decode(changed).is_err(), "variant {variant}");
    }
}

#[test]
fn capacity_recovery_result_requires_original_identity_and_no_admission_events() {
    let work = test_support::work();
    let mut agent = test_support::agent();
    agent.creation_key.push_str("-replacement");
    assert!(StoredRecovery::new(&agent, work.clone(), &[], test_support::NOW).is_err());
    let agent = test_support::agent();
    let event = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: agent.conversation.last_seq,
        appended_at: test_support::NOW.into(),
        kind: layer2_core::sandbox::AgentLogKind::StatusChanged {
            from: layer2_core::sandbox::AgentStatus::Failed,
            to: layer2_core::sandbox::AgentStatus::Ready,
        },
    };
    assert!(StoredRecovery::new(&agent, work, &[event], test_support::NOW).is_err());
}

#[test]
fn capacity_recovery_carrier_rejects_noncanonical_and_impossible_result_times() {
    for timestamp in [
        "2026-11-11T00:00:00+00:00",
        "2026-11-11T00:00:00.1Z",
        "2026-02-29T00:00:00Z",
        "2026-11-31T00:00:00Z",
        "2026-11-11T24:00:00Z",
    ] {
        let mut stored = test_support::stored();
        stored.result.published_at = timestamp.into();
        assert!(encode(&stored).is_err(), "encoded {timestamp}");
        let mut row = encode(&test_support::stored()).unwrap();
        row.get_document_mut("result")
            .unwrap()
            .insert("published_at", timestamp);
        assert!(decode(row).is_err(), "decoded {timestamp}");
    }
}
