use super::{provider_http::runtime_failure, reply::failure_error};
use crate::wire::{WireFailure, WireOperation};
use crate::AdapterError;

const PAIRS: [(&str, &str); 5] = [
    (
        "mutation_ledger_storage_hold",
        "runtime mutation ledger storage requires verification",
    ),
    (
        "mutation_ledger_busy",
        "runtime mutation ledger worker capacity is exhausted",
    ),
    (
        "mutation_ledger_writer_held",
        "runtime mutation ledger has another writer",
    ),
    (
        "mutation_ledger_format_hold",
        "runtime mutation ledger format requires verification",
    ),
    (
        "mutation_ledger_import_held",
        "imported runtime mutation ledger requires reviewed release authority",
    ),
];
fn failure(code: &str, detail: &str, retryable: bool) -> WireFailure {
    WireFailure {
        operation: WireOperation::RemoveAgentHome,
        code: code.into(),
        detail: detail.into(),
        retryable,
    }
}
#[test]
fn exact_indexed_holds_keep_pending_work_and_never_imply_runtime_loss_or_quota() {
    for (code, detail) in PAIRS {
        for retryable in [false, true] {
            for decode in [failure_error, runtime_failure] {
                let error = decode(failure(code, detail, retryable)).context("cleanup");
                assert!(matches!(error, AdapterError::RuntimeLedgerHold(_)));
                assert!(!error.is_runtime_loss());
                assert!(!matches!(error, AdapterError::AtCapacity(_)));
            }
        }
    }
}
#[test]
fn corrupt_or_near_match_failures_are_not_misclassified_as_indexed_holds() {
    for (code, detail) in PAIRS {
        for (code, detail) in [
            (format!("{code}_other"), detail.into()),
            (code.into(), format!("{detail} suffix")),
            (code.into(), "corrupt state".into()),
            ("mutation_ledger_invalid".into(), detail.into()),
            ("mutation_ledger_storage_invalid".into(), detail.into()),
        ] {
            for decode in [failure_error, runtime_failure] {
                assert!(matches!(
                    decode(failure(&code, &detail, false)),
                    AdapterError::Rejected(_)
                ));
            }
        }
    }
}
#[test]
fn indexed_holds_use_the_retained_nonretryable_wire_shape() {
    for (code, detail) in PAIRS {
        let wire = failure(code, detail, false);
        let bytes = serde_json::to_vec(&wire).unwrap();
        let decoded: WireFailure = serde_json::from_slice(&bytes).unwrap();
        assert!(!decoded.retryable);
        // Retained generic adapters reject these codes; they cannot infer a
        // replacement or quota. Upgraded adapters select durable hold backoff.
        assert_ne!(decoded.code, "at_capacity");
        assert!(matches!(
            runtime_failure(decoded),
            AdapterError::RuntimeLedgerHold(_)
        ));
    }
}
