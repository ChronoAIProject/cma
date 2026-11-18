use super::*;
use crate::sandbox::SandboxCredentialCustody;
use crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS;
use layer2_core::error::CoreError;
use layer2_core::sandbox::binding::{BindingCredentialCustody, BindingState, BindingWork};
use mongodb::Client;
use std::sync::Arc;
use std::time::Duration;

#[path = "tests/fixture.rs"]
mod fixture;
use fixture::Fixture;

async fn binding_recovery_accepts_current_parent_envelopes_without_changing_custody_case(
    schema: i64,
) {
    let fixture = Fixture::new(false, schema).await;
    let result = fixture
        .controller
        .retry_recovered_binding(&fixture.binding, &fixture.key)
        .await;
    let snapshot = fixture.snapshot().await;
    let retained = fixture
        .custody
        .get(&fixture.sandbox.id, 1)
        .await
        .unwrap()
        .unwrap();
    fixture.cleanup().await;
    let next = result.expect("ready retained parents with supported envelopes must recover");
    let mut expected = fixture.binding.clone();
    expected.state = BindingState::Ready;
    expected.advance_revision(next.updated_at.clone()).unwrap();
    assert_eq!(
        next, expected,
        "original generation and intent must be preserved"
    );
    assert_eq!(snapshot.0, next);
    assert_eq!(snapshot.1, fixture.sandbox);
    assert_eq!(snapshot.2, fixture.workspace);
    assert_eq!(snapshot.3, 1, "parent admission must commit exactly once");
    assert_eq!(retained.value.expose(), fixture.key.expose());
}

async fn binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent_case(
    command: &'static str,
) {
    recover_retry_case(command, None).await;
}

async fn recover_retry_case(command: &'static str, collection: Option<&'static str>) {
    let fixture = Fixture::new(true, 1).await;
    if let Some(collection) = collection {
        fixture.fault_at(command, collection);
    } else {
        fixture.fault(command, 1, false).await;
    }
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        fixture
            .controller
            .retry_recovered_binding(&fixture.binding, &fixture.key),
    )
    .await;
    let trace = fixture.release_fault().await;
    let snapshot = fixture.snapshot().await;
    fixture.cleanup().await;
    let next = result
        .expect("recovery must remain bounded")
        .expect("a known-aborted transaction must retry promptly");
    let mut expected = fixture.binding.clone();
    expected.state = BindingState::Ready;
    expected.advance_revision(next.updated_at.clone()).unwrap();
    assert_eq!(next, expected);
    assert_eq!(snapshot.0, next);
    assert_eq!(snapshot.1, fixture.sandbox);
    assert_eq!(snapshot.2, fixture.workspace);
    assert_eq!(snapshot.3, 1, "aborted parent writes must not accumulate");
    assert_eq!(snapshot.4, 1, "original binding fence must commit once");
    assert_eq!(
        trace.starts, 2,
        "one known abort needs exactly one fresh transaction"
    );
    assert_eq!(
        trace.known_abort_hits, 1,
        "the selected command must actually abort"
    );
    assert!(!trace.timestamps.is_empty());
    assert!(
        trace.timestamps.iter().all(|at| at == &next.updated_at),
        "retries must retain one original publication timestamp"
    );
}

async fn binding_recovery_bounds_known_aborts_and_never_replays_unknown_commit_case(unknown: bool) {
    let fixture = Fixture::new(true, 1).await;
    fixture
        .fault(
            if unknown {
                "commitTransaction"
            } else {
                "update"
            },
            0,
            unknown,
        )
        .await;
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        fixture.controller.recover_binding(&fixture.binding),
    )
    .await;
    let trace = fixture.release_fault().await;
    let snapshot = fixture.snapshot().await;
    fixture.cleanup().await;
    assert!(matches!(
        result.expect("recovery must remain bounded"),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert_eq!(
        trace.starts,
        if unknown { 1 } else { TRANSACTION_ATTEMPTS },
        "only a known-aborted transaction permits a fresh body"
    );
    assert_eq!(snapshot.0, fixture.binding);
    assert_eq!(snapshot.1, fixture.sandbox);
    assert_eq!(snapshot.2, fixture.workspace);
    assert_eq!(snapshot.3, 0);
    assert_eq!(snapshot.4, 0);
}

#[path = "tests_cases.rs"]
mod cases;

#[path = "tests/refusals.rs"]
mod refusals;
#[path = "tests/targeted.rs"]
mod targeted;
