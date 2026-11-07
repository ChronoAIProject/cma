use super::{provider_http::runtime_failure, reply::failure_error};
use crate::wire::{WireFailure, WireOperation};
use crate::AdapterError;

const CAPACITY_PAIRS: [(&str, &str); 4] = [
    (
        "mutation_authority_corrupt",
        "runtime mutation authority contains too many records",
    ),
    (
        "mutation_authority_corrupt",
        "mutation authority contains too many records",
    ),
    (
        "mutation_journal_corrupt",
        "runtime mutation journal contains too many entries",
    ),
    (
        "mutation_ledger_capacity",
        "runtime mutation ledger capacity is exhausted",
    ),
];

fn fixture(code: &str, detail: &str, retryable: bool) -> WireFailure {
    // Decode the actual retained wire shape rather than inventing a new field.
    serde_json::from_value(serde_json::json!({
        "operation": "remove_worktree", "code": code,
        "detail": detail, "retryable": retryable,
    }))
    .unwrap()
}

#[test]
fn ledger_holds_precede_retryability_in_both_transports_and_keep_context() {
    for (code, detail) in CAPACITY_PAIRS {
        for retryable in [false, true] {
            for map in [failure_error, runtime_failure] {
                let error = map(fixture(code, detail, retryable)).context("cleanup");
                assert!(
                    matches!(&error, AdapterError::RuntimeLedgerHold(text) if text.starts_with("cleanup: "))
                );
                assert!(!error.is_runtime_loss());
                assert!(!matches!(error, AdapterError::AtCapacity(_)));
            }
        }
    }
}

#[test]
fn only_exact_capacity_pairs_are_ledger_holds() {
    for (code, detail) in CAPACITY_PAIRS {
        let cases = [
            (code.into(), String::new()),
            (format!("{code}_suffix"), detail.into()),
            (format!("prefix_{code}"), detail.into()),
            (code.into(), format!("{detail} suffix")),
            (code.into(), format!("prefix {detail}")),
            (code.into(), format!("{detail}\n")),
            (code.into(), "malformed or conflicting record".into()),
            ("unrelated".into(), detail.into()),
        ];
        for (code, detail) in cases {
            for map in [failure_error, runtime_failure] {
                assert!(matches!(
                    map(fixture(&code, &detail, false)),
                    AdapterError::Rejected(_)
                ));
                assert!(matches!(
                    map(fixture(&code, &detail, true)),
                    AdapterError::Unavailable(_)
                ));
            }
        }
    }
    for map in [failure_error, runtime_failure] {
        assert!(matches!(
            map(fixture(
                "at_capacity",
                "native HTTP request capacity is exhausted",
                false
            )),
            AdapterError::AtCapacity(_)
        ));
        assert!(!matches!(
            map(fixture(
                "mutation_authority_conflict",
                CAPACITY_PAIRS[0].1,
                false
            )),
            AdapterError::RuntimeLedgerHold(_)
        ));
        assert!(matches!(
            map(fixture(
                "mutation_authority_corrupt",
                CAPACITY_PAIRS[2].1,
                false
            )),
            AdapterError::Rejected(_)
        ));
    }
}

#[test]
fn api_runtime_compatibility_keeps_old_nonretryable_fallback() {
    // The retained API's mapper below mirrors the pre-hold code. New runtime
    // capacity MUST remain retryable=false: old Starting paths may fail, but
    // cannot infer loss or quota. New APIs explicitly retain pending work.
    fn old_api(failure: WireFailure) -> AdapterError {
        if failure.retryable {
            AdapterError::Unavailable(failure.detail)
        } else if failure.code == "at_capacity" {
            AdapterError::AtCapacity(failure.detail)
        } else {
            AdapterError::Rejected(failure.detail)
        }
    }
    for (code, detail) in CAPACITY_PAIRS {
        let wire = fixture(code, detail, false);
        assert_eq!(wire.operation, WireOperation::RemoveWorktree);
        let old = old_api(wire.clone());
        assert!(matches!(old, AdapterError::Rejected(_)));
        assert!(!old.is_runtime_loss());
        for new_api in [failure_error, runtime_failure] {
            assert!(matches!(
                new_api(wire.clone()),
                AdapterError::RuntimeLedgerHold(_)
            ));
        }
    }
}
