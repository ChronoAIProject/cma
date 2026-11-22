use super::*;
use crate::sandbox::lifecycle_failure::{LifecycleAttempt, Origin, Phase};
use layer2_core::agent_ownership::OwnershipError;

fn failure(origin: Origin, error: SandboxStoreError) -> Result<bool, LifecycleTaskFailure> {
    Err(LifecycleAttempt::new(Phase::Execution, None).store(origin, error))
}

fn panic_text(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let panic = std::panic::catch_unwind(f).expect_err("worker error must fail");
    panic.downcast::<String>().map(|message| *message).unwrap()
}

#[test]
fn conflict_fails_at_first_attempt_with_scenario_origin_caller_and_counts() {
    for desktop in [false, true] {
        for origin in [Origin::Claim, Origin::Execute] {
            let retry = StoreRetry::new(desktop);
            let caller = Location::caller();
            let diagnostic = panic_text(|| {
                retry.check(
                    failure(origin, SandboxStoreError::Conflict),
                    "Sandbox",
                    caller,
                    "status=Installing step=Activate due_in_ms=Some(-1)",
                );
            });
            for expected in [
                "worker=Sandbox",
                "phase=execution",
                "error=Conflict",
                "unavailable_count=0",
                "conflict_count=1",
                "elapsed_ms=",
                "attempt_elapsed_ms=",
                "step=Activate",
                "due_in_ms=Some(-1)",
                "non-retryable worker failure",
            ] {
                assert!(diagnostic.contains(expected), "{diagnostic}");
            }
            assert!(diagnostic.contains(&format!("desktop={desktop}")));
            assert!(diagnostic.contains(&format!("origin={}", origin.as_str())));
            assert!(diagnostic.contains(&format!("callsite={caller}")));
        }
    }
}

#[test]
fn unavailable_budget_is_shared_and_fails_on_attempt_151() {
    let retry = StoreRetry::new(false);
    for index in 0..150 {
        assert!(retry
            .check(
                failure(
                    Origin::Execute,
                    SandboxStoreError::Unavailable("private detail".into())
                ),
                if index % 2 == 0 { "Agent" } else { "Sandbox" },
                Location::caller(),
                "state",
            )
            .is_none());
    }
    let diagnostic = panic_text(|| {
        retry.check(
            failure(
                Origin::Claim,
                SandboxStoreError::Unavailable("private detail".into()),
            ),
            "Agent",
            Location::caller(),
            "read_timeout",
        );
    });
    assert!(diagnostic.contains("unavailable_count=151"));
    assert!(diagnostic.contains("conflict_count=0"));
    assert!(diagnostic.contains("store unavailable retry budget exhausted"));
    assert!(!diagnostic.contains("private detail"));
}

#[test]
fn domain_corrupt_not_found_and_fencing_refusals_are_never_retried() {
    for error in [
        SandboxStoreError::NotFound,
        SandboxStoreError::SequenceConflict,
        SandboxStoreError::Corrupt("private detail".into()),
        SandboxStoreError::InvalidTransaction("private detail"),
    ] {
        let diagnostic = panic_text(|| {
            StoreRetry::new(false).check(
                failure(Origin::Execute, error.clone()),
                "Agent",
                Location::caller(),
                "record_missing",
            );
        });
        assert!(diagnostic.contains(&format!("error={}", error_class(&error))));
        assert!(diagnostic.contains("unavailable_count=0"));
        assert!(!diagnostic.contains("private detail"));
    }
    for error in [
        OwnershipError::Stale,
        OwnershipError::Invalid("private detail".into()),
    ] {
        let diagnostic = panic_text(|| {
            let failure = LifecycleAttempt::new(Phase::Execution, None)
                .ownership(Origin::AuthorityScope, error);
            StoreRetry::new(true).check::<bool>(Err(failure), "Agent", Location::caller(), "state");
        });
        assert!(diagnostic.contains("origin=authority_scope"));
        assert!(diagnostic.contains("cause=ownership_"));
        assert!(diagnostic.contains("conflict_count=1"));
        assert!(!diagnostic.contains("private detail"));
    }
}

#[test]
fn success_preserves_progress_without_consuming_the_retry_budget() {
    let retry = StoreRetry::new(false);
    for value in [false, true] {
        assert_eq!(
            retry.check(Ok(value), "Agent", Location::caller(), ""),
            Some(value)
        );
    }
    assert_eq!(retry.unavailable_retries.load(Ordering::SeqCst), 0);
    assert_eq!(retry.conflicts.load(Ordering::SeqCst), 0);
}
