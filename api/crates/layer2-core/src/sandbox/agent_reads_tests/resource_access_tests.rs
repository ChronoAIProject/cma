use super::*;
use crate::resource_access::*;
use sha2::{Digest as _, Sha256};

#[path = "resource_access_tests/service_tests.rs"]
mod service_tests;

async fn fixture() -> (InMemorySandboxStore, ResourceTarget) {
    let store = InMemorySandboxStore::default();
    insert_hierarchy(
        &store,
        "agent",
        "worktree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    (
        store,
        ResourceTarget::Agent {
            agent_id: AgentId("agent".into()),
        },
    )
}
fn request(snapshot: &ResourceAccessSnapshot, key: &str, mode: ShareMode) -> ShareMutation {
    let selector = match &snapshot.lineage.agent {
        Some(agent) => ResourceSelector::Agent {
            agent_id: agent.id.clone(),
        },
        None => ResourceSelector::Sandbox {
            sandbox_id: snapshot.lineage.sandbox.id.clone(),
        },
    };
    ShareMutation {
        selector,
        owner: ResourceOwner::Person {
            subject: snapshot.lineage.sandbox.owner.clone(),
        },
        actor: ResourceActor::Human {
            subject: snapshot.lineage.sandbox.owner.clone(),
        },
        key_sha256: format!("{:x}", Sha256::digest(key)),
        change: ShareChange::Create {
            target_org_id: SubjectId::new("org"),
            mode,
        },
        expected_share_revision: None,
        policy_fence: snapshot.fence(),
        proposed_id: format!("share-{key}"),
        at: NOW.into(),
    }
}
fn org_admin() -> CallerContext {
    let credential = CredentialContext {
        issuer: "https://identity.test".into(),
        subject: SubjectId::new("admin"),
        credential_id: "credential".into(),
    };
    let now = std::time::SystemTime::now();
    CallerContext {
        credential_source: crate::resource_access::CredentialSource::Retained,
        caller: caller("admin", true),
        credential: credential.clone(),
        actor: ResourceActor::Human {
            subject: SubjectId::new("admin"),
        },
        memberships: vec![OrganizationMembership::new(
            credential,
            SubjectId::new("org"),
            OrganizationRole::Admin,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )
        .unwrap()],
    }
}

#[tokio::test]
async fn snapshots_validate_each_resource_ancestry_without_inferring_owner_kind() {
    let (store, target) = fixture().await;
    for target in [
        target,
        ResourceTarget::Sandbox {
            sandbox_id: SandboxId("sandbox".into()),
        },
        ResourceTarget::Workspace {
            workspace_id: WorkspaceId("workspace".into()),
        },
        ResourceTarget::Worktree {
            worktree_id: WorktreeId("worktree".into()),
        },
    ] {
        let snapshot = store.resource_access_snapshot(&target).await.unwrap();
        assert_eq!(
            snapshot.scope(&target).unwrap().owner,
            ResourceOwner::Person {
                subject: SubjectId::new("owner")
            }
        );
        assert_eq!(snapshot.fence().policy_revision, 0);
        assert!(snapshot.policy.is_none());
    }
    assert_eq!(
        store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: AgentId("absent".into())
            })
            .await,
        Err(ResourceAccessStoreError::NotFound)
    );
}

#[tokio::test]
async fn share_create_replay_is_permanent_and_preserves_all_retained_resource_fields() {
    let (store, target) = fixture().await;
    let before = store.resource_access_snapshot(&target).await.unwrap();
    let first = request(&before, "create", ShareMode::View);
    let mut retry = first.clone();
    retry.proposed_id = "different-generated-id".into();
    let (a, b) = tokio::join!(store.mutate_share(first.clone()), store.mutate_share(retry));
    let receipt = a.unwrap();
    assert_eq!(receipt, b.unwrap());
    let after = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(before.lineage, after.lineage);
    assert_eq!(after.shares, std::slice::from_ref(&receipt.share));
    assert_eq!(after.fence().policy_revision, 1);
    let mut changed = first.clone();
    changed.change = ShareChange::Create {
        target_org_id: SubjectId::new("different"),
        mode: ShareMode::View,
    };
    assert_eq!(
        store.mutate_share(changed).await,
        Err(ResourceAccessStoreError::IdempotencyConflict)
    );
    let mut revoke = request(&after, "revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: receipt.share.id.clone(),
    };
    revoke.expected_share_revision = Some(1);
    let revoked = store.mutate_share(revoke).await.unwrap();
    assert_eq!(store.mutate_share(first).await.unwrap(), receipt);
    assert_eq!(
        store.resource_share(&revoked.share.id).await.unwrap(),
        Some(revoked.share)
    );
    let after = store.resource_access_snapshot(&target).await.unwrap();
    assert!(after.shares.is_empty());
    assert_eq!(after.lineage, before.lineage);
}

#[tokio::test]
async fn receipts_reject_changed_effects_attribution_and_root_identity() {
    let (store, target) = fixture().await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let command = request(&snapshot, "receipt", ShareMode::View);
    let receipt = store.mutate_share(command.clone()).await.unwrap();
    for tamper in 0..6 {
        let mut changed = receipt.clone();
        match tamper {
            0 => changed.share.target_org_id = SubjectId::new("other"),
            1 => changed.share.mode = ShareMode::OrgRoles,
            2 => changed.share.revision = 2,
            3 => {
                changed.share.issuer_actor = ResourceActor::Human {
                    subject: SubjectId::new("other"),
                }
            }
            4 => {
                changed.actor = ResourceActor::Human {
                    subject: SubjectId::new("other"),
                }
            }
            _ => changed.share.revoked_at = Some(changed.at.clone()),
        }
        assert_eq!(
            command.replay(changed),
            Err(ResourceAccessStoreError::Corrupt)
        );
    }
    let mut replaced = command.clone();
    replaced.policy_fence.sandbox_created_at = "2026-09-15T00:00:00Z".into();
    assert_eq!(
        replaced.replay(receipt),
        Err(ResourceAccessStoreError::IdempotencyConflict)
    );
}

#[tokio::test]
async fn stale_policy_and_share_revisions_cannot_overwrite_a_concurrent_revocation() {
    let (store, target) = fixture().await;
    let initial = store.resource_access_snapshot(&target).await.unwrap();
    let created = store
        .mutate_share(request(&initial, "create", ShareMode::View))
        .await
        .unwrap();
    let current = store.resource_access_snapshot(&target).await.unwrap();
    let mut update = request(&current, "update", ShareMode::View);
    update.change = ShareChange::SetMode {
        share_id: created.share.id.clone(),
        mode: ShareMode::View,
    };
    update.expected_share_revision = Some(1);
    let mut revoke = request(&current, "revoke", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: created.share.id.clone(),
    };
    revoke.expected_share_revision = Some(1);
    let (a, b) = tokio::join!(store.mutate_share(update), store.mutate_share(revoke));
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(
        matches!(a, Err(ResourceAccessStoreError::Conflict))
            || matches!(b, Err(ResourceAccessStoreError::Conflict))
    );
    assert_eq!(
        store
            .resource_share(&created.share.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        store
            .mutate_share(request(&initial, "stale", ShareMode::View))
            .await,
        Err(ResourceAccessStoreError::Conflict)
    );
}

#[tokio::test]
async fn narrow_interaction_requires_a_current_whole_sandbox_grant_and_never_widens_itself() {
    let (store, target) = fixture().await;
    let original = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(
        store
            .mutate_share(request(&original, "unsafe", ShareMode::OrgRoles))
            .await,
        Err(ResourceAccessStoreError::UnsafeInteractiveShare)
    );
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        original
    );
    let sandbox_target = ResourceTarget::Sandbox {
        sandbox_id: original.lineage.sandbox.id.clone(),
    };
    let whole_snapshot = store
        .resource_access_snapshot(&sandbox_target)
        .await
        .unwrap();
    let whole = store
        .mutate_share(request(&whole_snapshot, "whole", ShareMode::OrgRoles))
        .await
        .unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let narrow = store
        .mutate_share(request(&snapshot, "narrow", ShareMode::OrgRoles))
        .await
        .unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let admin = org_admin();
    assert!(AccessDecision::evaluate(
        &admin,
        &snapshot.scope(&target).unwrap(),
        &snapshot.shares,
        AccessPurpose::Read,
        std::time::SystemTime::now()
    )
    .unwrap()
    .allows(ResourceCapability::Interact));
    let whole_snapshot = store
        .resource_access_snapshot(&sandbox_target)
        .await
        .unwrap();
    let mut revoke = request(&whole_snapshot, "revoke-whole", ShareMode::View);
    revoke.change = ShareChange::Revoke {
        share_id: whole.share.id,
    };
    revoke.expected_share_revision = Some(1);
    store.mutate_share(revoke).await.unwrap();
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(snapshot.shares, [narrow.share]);
    assert!(!AccessDecision::evaluate(
        &admin,
        &snapshot.scope(&target).unwrap(),
        &snapshot.shares,
        AccessPurpose::Read,
        std::time::SystemTime::now()
    )
    .unwrap()
    .allows(ResourceCapability::Interact));
    assert_eq!(snapshot.lineage, original.lineage);
}

#[tokio::test]
async fn actor_owner_and_selector_tampering_are_refused_before_writes() {
    let (store, target) = fixture().await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let mut forged = request(&snapshot, "forged", ShareMode::View);
    forged.actor = ResourceActor::Human {
        subject: SubjectId::new("other"),
    };
    assert_eq!(
        store.mutate_share(forged).await,
        Err(ResourceAccessStoreError::Invalid)
    );
    let mut forged = request(&snapshot, "foreign", ShareMode::View);
    forged.actor = ResourceActor::Human {
        subject: SubjectId::new("other"),
    };
    forged.owner = ResourceOwner::Person {
        subject: SubjectId::new("other"),
    };
    assert_eq!(
        store.mutate_share(forged).await,
        Err(ResourceAccessStoreError::NotFound)
    );
    assert_eq!(
        store.resource_access_snapshot(&target).await.unwrap(),
        snapshot
    );
    for change in 0..5 {
        let mut corrupt = snapshot.clone();
        match change {
            0 => corrupt.lineage.workspace.as_mut().unwrap().owner = SubjectId::new("other"),
            1 => {
                corrupt.lineage.worktree.as_mut().unwrap().workspace_id =
                    WorkspaceId("other".into())
            }
            2 => corrupt.lineage.agent.as_mut().unwrap().sandbox_id = SandboxId("other".into()),
            3 => corrupt.lineage.worktree.as_mut().unwrap().agent_id = None,
            _ => corrupt.lineage.sandbox.owner = SubjectId::new("other"),
        }
        assert_eq!(
            corrupt.scope(&target),
            Err(ResourceAccessStoreError::Corrupt)
        );
    }
}

#[tokio::test]
async fn org_ownership_requires_explicit_evidence_bound_to_the_same_sandbox_incarnation() {
    let (store, target) = fixture().await;
    let mut snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let policy = ResourcePolicyRecord {
        sandbox_id: snapshot.lineage.sandbox.id.clone(),
        sandbox_created_at: snapshot.lineage.sandbox.created_at.clone(),
        owner: ResourceOwner::Organization {
            subject: snapshot.lineage.sandbox.owner.clone(),
        },
        organization_evidence: None,
        revision: 1,
    };
    snapshot.policy = Some(policy.clone());
    assert_eq!(
        snapshot.scope(&target),
        Err(ResourceAccessStoreError::Corrupt)
    );
    let mut policy = policy;
    policy.organization_evidence = Some(OrganizationOwnershipEvidence {
        issuer: "https://identity.test".into(),
        organization: snapshot.lineage.sandbox.owner.clone(),
        admitted_by: ResourceActor::Human {
            subject: SubjectId::new("admin"),
        },
        verified_at: NOW.into(),
    });
    snapshot.policy = Some(policy.clone());
    assert!(matches!(
        snapshot.scope(&target).unwrap().owner,
        ResourceOwner::Organization { .. }
    ));
    policy.sandbox_created_at = "2026-09-06T00:00:00Z".into();
    snapshot.policy = Some(policy);
    assert_eq!(
        snapshot.scope(&target),
        Err(ResourceAccessStoreError::Corrupt)
    );
    assert!(store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .policy
        .is_none());
}
