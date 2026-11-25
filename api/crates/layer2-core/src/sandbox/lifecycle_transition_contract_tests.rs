use std::fmt::Debug;

use super::*;
use crate::sandbox::store::{SandboxSessionStore, SandboxStoreError};

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
