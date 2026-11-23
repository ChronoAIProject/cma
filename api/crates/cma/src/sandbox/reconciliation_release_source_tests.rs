use super::*;
use crate::warm_pools::tests::runtime_pointer::released_source::{Call, ControlledSource};
use layer3_adapter::ReleasedRuntime;

#[tokio::test]
async fn daily_reconciliation_keeps_the_captured_runtime() {
    let (store, reconciler) = setup(0, Duration::ZERO).await;
    let retained = active_retained(0);
    store.create_sandbox(retained.clone()).await.unwrap();
    let source = Arc::new(ControlledSource::new(release(1, 'a')));
    source.advance_after_snapshot(release(2, 'b'));
    let reconciler = reconciler.with_released_runtime(source.clone());
    for (revision, digest) in [(1, 'a'), (2, 'b')] {
        let expected = release(revision, digest);
        reconciler
            .reconcile_runtime_update(&retained.id, SystemTime::now(), true)
            .await
            .unwrap();
        let row = store.get_sandbox(&retained.id).await.unwrap().unwrap();
        assert_eq!(
            row.lifecycle.runtime_update.unwrap().observed_version,
            expected.agent
        );
        assert_eq!(row.runtime, retained.runtime);
        assert_eq!(source.take_calls(), [Call::Snapshot(revision)]);
    }
}

fn active_retained(index: usize) -> Sandbox {
    let mut retained = candidate(index);
    retained.status = SandboxStatus::Active;
    retained.failure = None;
    assert!(retained.supports_retained_runtime_update());
    retained
}

fn release(revision: u64, digit: char) -> ReleasedRuntime {
    ReleasedRuntime {
        lane_revisions: None,
        revision,
        agent: format!(
            "registry.example/agent@sha256:{}",
            digit.to_string().repeat(64)
        ),
        machine: None,
    }
}

#[tokio::test]
async fn owner_and_operator_updates_keep_a_target_admitted_before_release_advances() {
    use layer2_core::{Authorization, Caller, Capability, SubjectId};

    for operator in [false, true] {
        let store = Arc::new(layer2_core::sandbox::store::InMemorySandboxStore::default());
        let retained = active_retained(0);
        let fresh = active_retained(1);
        for row in [&retained, &fresh] {
            store.create_sandbox(row.clone()).await.unwrap();
        }
        let first = release(1, 'a');
        let current = release(2, 'b');
        let source = Arc::new(ControlledSource::new(first.clone()));
        source.advance_after_validation(current.clone());
        let service = SandboxRuntimeUpdateService::with_released(store.clone(), source.clone());
        let caller = if operator {
            Caller::new(
                SubjectId::new("ops-test"),
                Authorization::new([Capability::ControlAnySandbox]),
            )
        } else {
            Caller::new(
                retained.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            )
        };
        assert_eq!(fresh.owner, retained.owner);

        for (sandbox, expected) in [(&retained, first), (&fresh, current)] {
            let updated = if operator {
                service
                    .request_as_operator(&caller, &sandbox.id, &expected.agent)
                    .await
            } else {
                service.request(&caller, &sandbox.id, &expected.agent).await
            }
            .unwrap();
            let saved = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
            assert_eq!(saved, updated);
            assert_eq!(saved.lifecycle.home, sandbox.lifecycle.home);
            assert_eq!(
                saved.runtime_update_request().unwrap().target_version,
                expected.agent
            );
            assert_eq!(
                source.take_calls(),
                [
                    Call::Snapshot(expected.revision),
                    Call::RequireCurrent(expected.revision, true)
                ],
            );
        }
    }
}
