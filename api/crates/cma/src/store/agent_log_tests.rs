use super::*;

#[test]
fn agent_log_index_is_unique_and_gap_detection_fails_closed() {
    let index = agent_log_indexes().remove(0);
    assert_eq!(index.keys, doc! { "agent_id": 1, "seq": 1 });
    assert_eq!(index.options.unwrap().unique, Some(true));
    let id = AgentId("agt_test".into());
    let entry = |seq| AgentLogEntry {
        agent_id: id.clone(),
        seq,
        appended_at: "2026-09-05T00:00:00Z".into(),
        kind: AgentLogKind::Stopped {
            reason: "done".into(),
        },
    };
    assert!(verify_contiguous(1, 3, 2, &[entry(2), entry(3)]).is_ok());
    assert_eq!(
        verify_contiguous(1, 3, 2, &[entry(3)]),
        Err(SandboxStoreError::SequenceConflict)
    );
    assert!(verify_complete_log(&id, 3, &[entry(1), entry(2), entry(3)]).is_ok());
    assert_eq!(
        verify_complete_log(&id, 3, &[entry(1), entry(3)]),
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        verify_complete_log(&id, 3, &[entry(1), entry(2), entry(3), entry(4)]),
        Err(SandboxStoreError::SequenceConflict)
    );
    let mut foreign = entry(2);
    foreign.agent_id = AgentId("agt_foreign".into());
    assert_eq!(
        verify_complete_log(&id, 3, &[entry(1), foreign, entry(3)]),
        Err(SandboxStoreError::SequenceConflict)
    );
}
