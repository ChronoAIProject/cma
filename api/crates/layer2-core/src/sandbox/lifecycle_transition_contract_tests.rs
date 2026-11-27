use std::fmt::Debug;

use super::*;
use crate::sandbox::store::{SandboxSessionStore, SandboxStoreError, WorkspaceWorkerStore};

#[path = "lifecycle_transition_contract_tests/workspace_prerequisites_tests.rs"]
mod workspace_prerequisites_tests;

const TRANSITION_AT: &str = "2026-09-05T00:01:00Z";

use SandboxStatus as S;
use WorkspaceStatus as W;
use WorktreeStatus as T;

// Include terminal rows so they are also tested as destinations and sources.
const SANDBOX_EDGES: &[(S, &[S])] = &[
    (
        S::Provisioning,
        &[S::Installing, S::Reviving, S::Failed, S::Retiring],
    ),
    (
        S::Installing,
        &[S::Active, S::Reviving, S::Failed, S::Retiring],
    ),
    (
        S::Active,
        &[
            S::Pausing,
            S::Reviving,
            S::CredentialRotating,
            S::Rotating,
            S::Failed,
            S::Retiring,
        ],
    ),
    (
        S::Pausing,
        &[S::Paused, S::Active, S::Reviving, S::Failed, S::Retiring],
    ),
    (
        S::Paused,
        &[S::Reviving, S::Rotating, S::Failed, S::Retiring],
    ),
    (
        S::Reviving,
        &[S::Pausing, S::Active, S::Failed, S::Retiring],
    ),
    (
        S::CredentialRotating,
        &[S::Reviving, S::Active, S::Failed, S::Retiring],
    ),
    (
        S::Rotating,
        &[S::Pausing, S::Paused, S::Active, S::Failed, S::Retiring],
    ),
    (S::Failed, &[S::Retiring]),
    (S::Retiring, &[S::Retired, S::Failed]),
    (S::Retired, &[]),
];

const WORKSPACE_EDGES: &[(W, &[W])] = &[
    (W::Preparing, &[W::Ready, W::Failed, W::Removing]),
    (W::Ready, &[W::Removing]),
    (W::Failed, &[W::Removing]),
    (W::Removing, &[W::Removed, W::Failed]),
    (W::Removed, &[]),
];

const WORKTREE_EDGES: &[(T, &[T])] = &[
    (T::Creating, &[T::Ready, T::Failed]),
    (T::Ready, &[T::Removing]),
    (T::Failed, &[T::Removing]),
    (T::Removing, &[T::Removed]),
    (T::Removed, &[]),
];

fn check_state_pairs<State: Copy + Debug + Eq, Aggregate: Clone + Debug + Eq>(
    graph: &[(State, &[State])],
    resource: &'static str,
    name: impl Fn(State) -> &'static str,
    fixture: impl Fn(State) -> Aggregate,
    transition: impl Fn(&mut Aggregate, State) -> Result<(), SandboxModelError>,
    expected_change: impl Fn(&mut Aggregate, State),
) {
    for &(source, successors) in graph {
        let original = fixture(source);
        for &(target, _) in graph {
            let mut actual = original.clone();
            let result = transition(&mut actual, target);
            let mut expected = original.clone();
            let expected_result = if successors.contains(&target) {
                expected_change(&mut expected, target);
                Ok(())
            } else {
                Err(SandboxModelError::InvalidTransition {
                    resource,
                    from: name(source),
                    to: name(target),
                })
            };
            assert_eq!(
                result, expected_result,
                "{resource} {source:?} -> {target:?}"
            );
            assert_eq!(actual, expected, "{resource} {source:?} -> {target:?}");
        }
    }
}

fn fixture_failure() -> TypedFailure {
    TypedFailure {
        class: "fixture_failure".into(),
        detail: "Synthetic lifecycle failure".into(),
    }
}

fn sandbox_in(status: S) -> Sandbox {
    let mut value = sandbox();
    value.status = status;
    value.failure = (status == S::Failed).then(fixture_failure);
    value.retired_at = (status == S::Retired).then(|| NOW.into());
    value
}

#[test]
fn sandbox_lifecycle_matches_the_transition_contract() {
    check_state_pairs(
        SANDBOX_EDGES,
        "sandbox",
        S::name,
        sandbox_in,
        |value, target| {
            value.transition(
                target,
                TRANSITION_AT.into(),
                (target == S::Failed).then(fixture_failure),
            )
        },
        |expected, target| {
            expected.status = target;
            expected.revision += 1;
            expected.updated_at = TRANSITION_AT.into();
            expected.failure = (target == S::Failed).then(fixture_failure);
            expected.retired_at = (target == S::Retired).then(|| TRANSITION_AT.into());
        },
    );
}

#[test]
fn workspace_lifecycle_matches_the_transition_contract() {
    check_state_pairs(
        WORKSPACE_EDGES,
        "workspace",
        W::name,
        |status| {
            let mut value = workspace();
            value.status = status;
            value.ready_at = (status == W::Ready).then(|| NOW.into());
            value.failed = (status == W::Failed).then(fixture_failure);
            value.removed_at = (status == W::Removed).then(|| NOW.into());
            value
        },
        |value, target| {
            value.transition(
                target,
                TRANSITION_AT.into(),
                (target == W::Failed).then(fixture_failure),
            )
        },
        |expected, target| {
            expected.status = target;
            expected.revision += 1;
            expected.failed = (target == W::Failed).then(fixture_failure);
            if target == W::Ready {
                expected.ready_at = Some(TRANSITION_AT.into());
            }
            if target == W::Removed {
                expected.removed_at = Some(TRANSITION_AT.into());
            }
        },
    );
}

#[test]
fn worktree_lifecycle_matches_the_transition_contract() {
    check_state_pairs(
        WORKTREE_EDGES,
        "worktree",
        T::name,
        |status| Worktree {
            status,
            ..worktree()
        },
        Worktree::transition,
        |expected, target| {
            expected.status = target;
            expected.revision += 1;
        },
    );
}

#[test]
fn abandoned_worktree_removal_requires_an_unfinished_agent_worktree() {
    let ready = worktree();
    let unfinished = Worktree {
        status: T::Creating,
        ..ready.clone()
    };
    for original in [
        ready,
        Worktree {
            agent_id: None,
            ..unfinished.clone()
        },
    ] {
        let mut refused = original.clone();
        assert_eq!(
            refused.remove_abandoned_agent_worktree(),
            Err(SandboxModelError::invalid_value("worktree"))
        );
        assert_eq!(refused, original);
    }

    let mut exhausted = Worktree {
        revision: u64::MAX,
        ..unfinished.clone()
    };
    let original = exhausted.clone();
    assert_eq!(
        exhausted.remove_abandoned_agent_worktree(),
        Err(SandboxModelError::invalid_value("worktree.revision"))
    );
    assert_eq!(exhausted, original);

    let mut admitted = unfinished.clone();
    admitted.remove_abandoned_agent_worktree().unwrap();
    assert_eq!(
        admitted,
        Worktree {
            status: T::Removing,
            revision: unfinished.revision + 1,
            ..unfinished
        }
    );
}

#[tokio::test]
async fn workspace_renewal_rejects_expired_and_replaced_claims_without_mutation() {
    const FIRST_EXPIRY: &str = "2026-09-05T00:00:15Z";
    const RENEWED_AT: &str = "2026-09-05T00:00:05Z";
    const RENEWED_EXPIRY: &str = "2026-09-05T00:00:20Z";
    const NEXT_EXPIRY: &str = "2026-09-05T00:00:35Z";

    let store = InMemorySandboxStore::default();
    let parent = sandbox_in(S::Active);
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut unclaimed = workspace();
    unclaimed.status = W::Preparing;
    unclaimed.ready_at = None;
    unclaimed.resolved_commit = None;
    unclaimed
        .work
        .schedule(WorkspaceWorkStep::Prepare, NOW.into());
    store.create_workspace(unclaimed.clone()).await.unwrap();

    assert_eq!(
        store
            .renew_workspace_authority(&unclaimed, NOW, FIRST_EXPIRY)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    let first = store
        .claim_workspace_work(&unclaimed.id, "first", NOW.into(), FIRST_EXPIRY.into())
        .await
        .unwrap()
        .unwrap();
    let renewal = store
        .renew_workspace_authority(&first, RENEWED_AT, RENEWED_EXPIRY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewal.expires_at, RENEWED_EXPIRY);
    assert_eq!(renewal.valid_for, std::time::Duration::from_secs(15));

    let mut expected = first.clone();
    expected.work.lease.as_mut().unwrap().expires_at = RENEWED_EXPIRY.into();
    assert_eq!(
        store.get_workspace(&first.id).await.unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        store
            .renew_workspace_authority(&first, RENEWED_EXPIRY, NEXT_EXPIRY)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store.get_workspace(&first.id).await.unwrap(),
        Some(expected)
    );

    let successor = store
        .claim_workspace_work(
            &first.id,
            "successor",
            RENEWED_EXPIRY.into(),
            NEXT_EXPIRY.into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        successor.work.lease.as_ref().unwrap().fence > first.work.lease.as_ref().unwrap().fence
    );
    assert_eq!(
        store
            .renew_workspace_authority(&first, RENEWED_EXPIRY, NEXT_EXPIRY)
            .await
            .unwrap(),
        None
    );
    let mut old_fence = successor.clone();
    old_fence.work.lease.as_mut().unwrap().fence = first.work.lease.as_ref().unwrap().fence;
    let mut wrong_step = successor.clone();
    wrong_step.work.step = WorkspaceWorkStep::Remove;
    let mut wrong_owner = successor.clone();
    wrong_owner.owner = crate::SubjectId::new("other-owner");
    let mut wrong_parent = successor.clone();
    wrong_parent.sandbox_id = SandboxId("sbx_other".into());
    let mut missing = successor.clone();
    missing.id = crate::WorkspaceId("wks_missing".into());
    for observed in [old_fence, wrong_step, wrong_owner, wrong_parent, missing] {
        assert_eq!(
            store
                .renew_workspace_authority(&observed, RENEWED_EXPIRY, NEXT_EXPIRY)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store.get_workspace(&first.id).await.unwrap(),
            Some(successor.clone())
        );
    }

    let mut rotating = parent.clone();
    rotating
        .transition(S::Rotating, RENEWED_EXPIRY.into(), None)
        .unwrap();
    store
        .save_sandbox(parent.revision, rotating.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .renew_workspace_authority(&successor, RENEWED_EXPIRY, NEXT_EXPIRY)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store.get_workspace(&first.id).await.unwrap(),
        Some(successor)
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(rotating));
}

#[tokio::test]
async fn only_active_sandbox_admits_a_live_session() {
    for &(status, _) in SANDBOX_EDGES {
        let store = InMemorySandboxStore::default();
        let parent = sandbox_in(status);
        store.create_sandbox(parent.clone()).await.unwrap();
        let prior_event = store
            .append_sandbox_event(
                &parent.id,
                NOW.into(),
                SandboxEventKind::CredentialChanged { key_generation: 1 },
            )
            .await
            .unwrap();

        let admitted = store
            .open_sandbox_session(&parent.id, "tc037-session", "2026-09-05T00:02:00Z")
            .await;
        if status == S::Active {
            admitted.unwrap();
        } else {
            assert!(
                matches!(admitted, Err(SandboxStoreError::Conflict)),
                "{status:?}"
            );
        }
        assert_eq!(
            store
                .has_open_sandbox_session(&parent.id, TRANSITION_AT)
                .await
                .unwrap(),
            status == S::Active,
            "{status:?}",
        );
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        assert_eq!(
            store.sandbox_events(&parent.id, 0, 10).await.unwrap(),
            vec![prior_event],
            "{status:?}",
        );
    }
}
