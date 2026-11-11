use super::*;

fn entry(seq: u64, kind: AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: crate::ids::AgentId::new("agt_pending"),
        seq,
        appended_at: "2026-11-11T00:00:00Z".into(),
        kind,
    }
}

fn request(seq: u64, id: &str) -> AgentLogEntry {
    entry(
        seq,
        AgentLogKind::ApprovalRequested {
            approval_id: id.into(),
            action: Some("Check".into()),
            detail: None,
            runtime_locator: Some(format!("route-{seq}")),
        },
    )
}

#[test]
fn reset_suffix_preserves_exact_pending_requests_for_every_boundary() {
    for boundary in [
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
        AgentLogKind::TurnFailed {
            turn_sequence: 1,
            failure: crate::sandbox::TypedFailure {
                class: "failed".into(),
                detail: "test".into(),
            },
        },
        AgentLogKind::TurnInterrupted { turn_sequence: 1 },
        AgentLogKind::Stopped {
            reason: "stopped".into(),
        },
        AgentLogKind::Deleted,
    ] {
        let entries = vec![
            request(1, "old"),
            request(2, "z"),
            entry(3, boundary),
            request(4, "z"),
            request(5, "a"),
            entry(
                6,
                AgentLogKind::ApprovalResolved {
                    approval_id: "z".into(),
                    decision: "approved".into(),
                },
            ),
            request(7, "z"),
        ];
        let mut full = PendingAgentApprovals::default();
        for entry in &entries {
            full.observe(entry);
        }
        let expected = vec![entries[4].clone(), entries[6].clone()];
        assert_eq!(full.into_entries(), expected);
        for after in 0..=2 {
            assert_eq!(
                PendingAgentApprovals::from_reset_suffix(&entries[after..]),
                Some(expected.clone())
            );
        }
        assert_eq!(
            PendingAgentApprovals::from_reset_suffix(&entries[3..]),
            None
        );
    }
}

#[test]
fn a_suffix_without_a_reset_cannot_prove_absence_of_older_requests() {
    let entries = [
        request(20, "new"),
        entry(
            21,
            AgentLogKind::ApprovalResolved {
                approval_id: "new".into(),
                decision: "denied".into(),
            },
        ),
    ];
    assert_eq!(PendingAgentApprovals::from_reset_suffix(&entries), None);
    assert_eq!(PendingAgentApprovals::from_reset_suffix(&[]), None);
    let mut entries = entries.to_vec();
    entries.push(entry(22, AgentLogKind::Deleted));
    assert_eq!(
        PendingAgentApprovals::from_reset_suffix(&entries),
        Some(vec![])
    );
}
