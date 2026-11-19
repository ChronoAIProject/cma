use super::*;
use layer2_core::sandbox::TypedFailure;

#[test]
fn provider_failure_projection_preserves_only_exact_adjacent_deferred_proof() {
    use layer2_core::sandbox::agent_io::SOURCE_DEFERRED_FAILURE;
    for fault in [
        "none",
        "fingerprint",
        "adjacency",
        "timestamp",
        "terminal",
        "class",
    ] {
        for personal in [false, true] {
            let mut value = reading(Some(2));
            value.command.status = if fault == "terminal" {
                AgentResponseStatus::Failed
            } else {
                AgentResponseStatus::InProgress
            };
            value.deferred_failure_detail = Some("PRIVATE_PROOF_SENTINEL".into());
            let mut history = snapshot(vec![
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 1,
                    operation: AgentOperationKind::SendMessage,
                    state: AgentOperationDelivery::DispatchFenced,
                },
                start(2, None),
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 1,
                    operation: AgentOperationKind::SendMessage,
                    state: AgentOperationDelivery::PreWriteFailed,
                },
                AgentLogKind::TurnFailed {
                    turn_sequence: 2,
                    failure: TypedFailure {
                        class: if fault == "class" {
                            "agent_turn_failed"
                        } else {
                            SOURCE_DEFERRED_FAILURE
                        }
                        .into(),
                        detail: if fault == "fingerprint" {
                            "PRIVATE_OTHER_SENTINEL"
                        } else {
                            "PRIVATE_PROOF_SENTINEL"
                        }
                        .into(),
                    },
                },
            ]);
            if fault == "adjacency" {
                history.history.last_mut().unwrap().seq += 1;
            }
            if fault == "timestamp" {
                history.history.last_mut().unwrap().appended_at = "2026-10-15T00:00:01Z".into();
            }
            for entry in &mut history.history {
                *entry = AgentVisibleLogEntry::from_entry(
                    AgentLogEntry {
                        agent_id: history.id.clone(),
                        seq: entry.seq,
                        appended_at: entry.appended_at.clone(),
                        kind: entry.kind.clone().unwrap(),
                    },
                    personal,
                );
            }
            let projected = project(value, &history).unwrap();
            assert_eq!(
                projected.status,
                if fault == "none" {
                    crate::responses::v2::V2ResponseStatus::Queued
                } else {
                    crate::responses::v2::V2ResponseStatus::Failed
                },
                "{fault} personal={personal}"
            );
            assert_eq!(projected.error.is_none(), fault == "none");
            assert!(!serde_json::to_string(&events(&projected, true))
                .unwrap()
                .contains("SENTINEL"));
        }
    }
}

#[test]
fn retained_provider_failure_is_safe_in_raw_v2_response_and_stream() {
    for class in ["agent_turn_failed", "PRIVATE_CLASS_SENTINEL"] {
        let history = snapshot(vec![start(1, None), AgentLogKind::TurnFailed {
            turn_sequence: 1, failure: TypedFailure {
                class: class.into(), detail: "unexpected status 503: <html>PRIVATE_BODY_SENTINEL https://failure.invalid X-Private-Header</html>".into(),
            },
        }]);
        let projected = project(reading(Some(1)), &history).unwrap();
        assert_eq!(
            projected.status,
            crate::responses::v2::V2ResponseStatus::Failed
        );
        for wire in [
            serde_json::to_string(&projected).unwrap(),
            serde_json::to_string(&events(&projected, true)).unwrap(),
        ] {
            for private in [
                "PRIVATE_BODY_SENTINEL",
                "PRIVATE_CLASS_SENTINEL",
                "failure.invalid",
                "X-Private-Header",
            ] {
                assert!(
                    !wire.contains(private),
                    "public payload exposed {private}: {wire}"
                );
            }
            assert!(wire.contains("agent_turn_failed"));
            assert!(wire.contains("The Agent run failed."));
        }
        assert!(
            matches!(&history.history[1].kind, Some(AgentLogKind::TurnFailed { failure, .. }) if failure.detail.contains("PRIVATE_BODY_SENTINEL"))
        );
    }
}
