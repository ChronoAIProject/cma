use super::*;
use layer2_core::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};

fn seal() -> ChannelReuseCleanupSeal {
    ChannelReuseCleanupSeal {
        allocation_id: format!("bral_{}", "1".repeat(64)),
        owner: ResourceOwner::Person {
            subject: SubjectId::new("owner"),
        },
        target: BridgeAgentTarget {
            sandbox_id: SandboxId("parent".into()),
            workspace_id: WorkspaceId("shared".into()),
            worktree_id: WorktreeId("child-tree".into()),
            agent_id: AgentId("child-agent".into()),
        },
        sandbox_created_at: "2026-09-01T00:00:00Z".into(),
        workspace_created_at: "2026-09-01T00:01:00Z".into(),
        agent_created_at: "2026-09-01T00:02:00Z".into(),
        runtime: RuntimeBinding {
            boot_id: Some("original-boot".into()),
            runtime_id: "ordinary-runtime".into(),
            runtime_version: "v1".into(),
            provision_generation: Some(3),
            bound_at: "2026-09-01T00:00:00Z".into(),
        },
        workspace_path: "/home/cma/shared".into(),
        worktree_path: "/home/cma/shared/.worktrees/wt-1".into(),
        home_path: "/home/cma/.cma/agents/child-agent/home".into(),
        workspace_owned: false,
        allocation_sha256: "2".repeat(64),
        creation_sha256: "3".repeat(64),
        startup_sha256: "4".repeat(64),
        workspace_creation_sha256: None,
    }
}
fn state(holds: Vec<ChannelReuseCleanupHold>) -> ChannelReuseCleanupState {
    let seal = seal();
    ChannelReuseCleanupState::new(
        ChannelReuseCleanupRequest {
            request_id: "explicit-request-1".into(),
            allocation_id: seal.allocation_id.clone(),
            expected_allocation_revision: 8,
        },
        ResourceActor::Human {
            subject: SubjectId::new("owner"),
        },
        seal,
        holds,
        1234,
    )
    .unwrap()
}

#[test]
fn retained_request_preserves_uncertain_effects_and_rejects_tampering() {
    use ChannelReuseCleanupHold::*;
    let original = state(vec![
        PendingCommand,
        UncertainCommand,
        PendingReply,
        UncertainReply,
    ]);
    let bytes = serde_json::to_vec(&original).unwrap();
    let recovered: ChannelReuseCleanupState = serde_json::from_slice(&bytes).unwrap();
    assert!(recovered == original);
    recovered.validate().unwrap();
    let mut changed = recovered.clone();
    changed.holds.clear();
    assert!(changed.validate().is_err());
    let mut changed = recovered.clone();
    changed.request.request_id = "different-request".into();
    assert!(changed.validate().is_err());
    let mut changed = recovered;
    changed.seal.runtime.provision_generation = Some(4);
    assert!(changed.validate().is_err());
}

#[test]
fn whole_workspace_claim_requires_retained_workspace_creation_evidence() {
    let original = seal();
    original.validate().unwrap();
    let mut changed = original.clone();
    changed.workspace_owned = true;
    assert!(changed.validate().is_err());
    changed.workspace_creation_sha256 = Some("5".repeat(64));
    changed.validate().unwrap();
    changed.workspace_owned = false;
    assert!(changed.validate().is_err());
    let mut changed = original;
    changed.runtime.boot_id = None;
    assert!(changed.validate().is_err());
}

#[test]
fn paths_cannot_expand_from_exact_child_to_siblings_or_shared_home() {
    for path in [
        "/home/cma",
        "/home/cma/shared/..",
        "/home/cma/shared/child",
        "/home/cma//shared",
    ] {
        let mut changed = seal();
        changed.workspace_path = path.into();
        assert!(changed.validate().is_err(), "workspace path {path}");
    }
    for path in [
        "/home/cma/shared",
        "/home/cma/other/.worktrees/wt-1",
        "/home/cma/shared/.worktrees/wt-01",
        "/home/cma/shared/.worktrees/wt-1/../wt-2",
    ] {
        let mut changed = seal();
        changed.worktree_path = path.into();
        assert!(changed.validate().is_err(), "worktree path {path}");
    }
    for path in [
        "/home/cma",
        "/home/cma/.cma/agents",
        "/home/cma/.cma/agents/sibling/home",
    ] {
        let mut changed = seal();
        changed.home_path = path.into();
        assert!(changed.validate().is_err(), "home path {path}");
    }
}

#[test]
fn checksum_cannot_make_malformed_request_or_scope_valid() {
    let mut changed = state(Vec::new());
    changed.seal.home_path = "/home/cma".into();
    changed.digest = changed.compute_digest().unwrap();
    assert!(changed.validate().is_err());
    let mut changed = state(Vec::new());
    changed.request.request_id.clear();
    changed.digest = changed.compute_digest().unwrap();
    assert!(changed.validate().is_err());
    let mut changed = state(Vec::new());
    changed.holds = vec![
        ChannelReuseCleanupHold::UncertainReply,
        ChannelReuseCleanupHold::PendingCommand,
    ];
    changed.digest = changed.compute_digest().unwrap();
    assert!(changed.validate().is_err());
}

#[test]
fn admission_closure_binds_original_request_actor_and_child_without_settling_holds() {
    let request = state(vec![ChannelReuseCleanupHold::PendingCommand]);
    let closure = ChannelReuseCleanupAdmission::new(
        &request,
        request.requested_by.clone(),
        request.requested_at_ms + 1,
    )
    .unwrap();
    let restored: ChannelReuseCleanupAdmission =
        serde_json::from_slice(&serde_json::to_vec(&closure).unwrap()).unwrap();
    restored.require(&request).unwrap();
    let mut changed = restored.clone();
    changed.agent_id = AgentId("sibling".into());
    assert!(changed.require(&request).is_err());
    let mut changed = restored;
    changed.request_sha256 = "0".repeat(64);
    assert!(changed.require(&request).is_err());
    assert!(ChannelReuseCleanupAdmission::new(
        &request,
        ResourceActor::Human {
            subject: SubjectId::new("other")
        },
        request.requested_at_ms + 1,
    )
    .is_err());
    assert_eq!(request.holds, vec![ChannelReuseCleanupHold::PendingCommand]);
}
