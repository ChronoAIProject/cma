use super::command_ledger_tests::execution_tests::resource_execution::revival::revive_managed_sandbox;
use crate::warm_pools::tests::runtime_pointer::released_source::{Call, ControlledSource};
use layer3_adapter::ReleasedRuntime;
use std::sync::Arc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn command_revival_recheck_uses_the_captured_runtime() {
    let first = release(1, 'b');
    let current = release(2, 'c');
    let source = Arc::new(ControlledSource::new(first.clone()));
    source.advance_after_snapshot(current.clone());
    let (revived, original) = revive_managed_sandbox(source.clone()).await;
    assert_eq!(revived.lifecycle.home, original.lifecycle.home);
    assert_eq!(
        revived.runtime_update_request().unwrap().target_version,
        first.agent
    );
    assert_eq!(snapshot_reads(&source), [Call::Snapshot(first.revision)]);

    let (revived, _) = revive_managed_sandbox(source.clone()).await;
    assert_eq!(
        revived.runtime_update_request().unwrap().target_version,
        current.agent
    );
    assert_eq!(snapshot_reads(&source), [Call::Snapshot(current.revision)]);
}

fn release(revision: u64, digest: char) -> ReleasedRuntime {
    ReleasedRuntime {
        lane_revisions: None,
        revision,
        agent: format!(
            "registry.example/runtime@sha256:{}",
            digest.to_string().repeat(64)
        ),
        machine: None,
    }
}

fn snapshot_reads(source: &ControlledSource) -> Vec<Call> {
    source
        .take_calls()
        .into_iter()
        .filter(|call| matches!(call, Call::Snapshot(_)))
        .collect()
}
