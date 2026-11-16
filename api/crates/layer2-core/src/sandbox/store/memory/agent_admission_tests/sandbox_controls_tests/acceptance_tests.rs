//! Concurrent receipt acceptance and application preserve the live hierarchy.
use super::*;

#[tokio::test]
async fn sandbox_controls_concurrent_acceptance_and_application_preserve_live_tree() {
    let f = Fixture::new().await;
    f.store.admit_agent(admission("keep-agent")).await.unwrap();
    let before = f.store.lock().clone();
    let revision = before.sandboxes[&sandbox(4).id].revision;
    let (left, right) = tokio::join!(
        f.admit("owner", SandboxControl::Pause, "retry", revision),
        f.admit("owner", SandboxControl::Pause, "retry", revision)
    );
    let record = left.unwrap();
    assert_eq!(record, right.unwrap());
    assert!(matches!(
        f.admit("owner", SandboxControl::Revive, "other", revision)
            .await,
        Err(SandboxControlServiceError::Store(SandboxControlError::Busy))
    ));
    let proof = f.proof("owner", &record).await;
    let (left, right) = tokio::join!(
        f.store.apply_sandbox_control(&record, proof.clone()),
        f.store.apply_sandbox_control(&record, proof)
    );
    let applied = left.unwrap();
    assert_eq!(right, Err(SandboxControlError::Conflict));
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied {
            disposition: SandboxControlDisposition::Scheduled,
            ..
        }
    ));
    assert_eq!(
        f.admit("owner", SandboxControl::Pause, "retry", revision)
            .await
            .unwrap(),
        applied
    );
    assert!(matches!(
        f.admit("owner", SandboxControl::Pause, "retry", revision + 1)
            .await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    let after = f.store.lock();
    let root = &after.sandboxes[&sandbox(4).id];
    assert_eq!(root.status, SandboxStatus::Pausing);
    assert_eq!(root.runtime, before.sandboxes[&root.id].runtime);
    assert_eq!(root.profile, before.sandboxes[&root.id].profile);
    assert_eq!(root.credential, before.sandboxes[&root.id].credential);
    assert_eq!(root.owner, before.sandboxes[&root.id].owner);
    assert_eq!(after.agents, before.agents);
    assert_eq!(after.agent_logs, before.agent_logs);
    assert_eq!(after.worktrees, before.worktrees);
    assert_eq!(after.workspaces, before.workspaces);
    assert_eq!(
        after.agent_creation_provenance,
        before.agent_creation_provenance
    );
    assert_eq!(
        after.sandbox_events[&root.id].len(),
        before.sandbox_events.get(&root.id).map_or(0, Vec::len) + 1
    );
    assert_eq!(after.sandbox_controls.len(), 1);
}
