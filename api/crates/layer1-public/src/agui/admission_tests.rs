//! Focused checks for AG-UI's finite operation metadata.

use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Example {
    thread: String,
    sequence: u64,
}

#[test]
fn finite_outcomes_are_versioned_and_round_trip() {
    let outcome = Example {
        thread: "sesn_effective".into(),
        sequence: 41,
    };
    let encoded = encode_outcome(&outcome).expect("the outcome encodes");
    let wire: serde_json::Value = serde_json::from_slice(&encoded).expect("outcome is JSON");

    assert_eq!(wire["version"], OUTCOME_VERSION);
    assert_eq!(decode_outcome::<Example>(&encoded).unwrap(), outcome);
}

#[test]
fn another_outcome_version_is_never_guessed_at() {
    let encoded = serde_json::json!({
        "version": OUTCOME_VERSION + 1,
        "outcome": { "thread": "sesn_future", "sequence": 7 }
    });
    assert!(matches!(
        decode_outcome::<Example>(&serde_json::to_vec(&encoded).unwrap()),
        Err(RunFailure::Store(_))
    ));
}

#[test]
fn semantic_fingerprints_are_stable_and_field_sensitive() {
    let first = Example {
        thread: "sesn_1".into(),
        sequence: 3,
    };
    let same = Example {
        thread: "sesn_1".into(),
        sequence: 3,
    };
    let changed = Example {
        thread: "sesn_1".into(),
        sequence: 4,
    };

    assert_eq!(fingerprint(&first).unwrap(), fingerprint(&same).unwrap());
    assert_ne!(fingerprint(&first).unwrap(), fingerprint(&changed).unwrap());
}

#[test]
fn a_stored_problem_supplies_every_required_run_error_field() {
    let problem = StoredProblem::provisioning_timeout();
    let (message, code, capability) = problem.stream_fields();

    assert!(!message.is_empty());
    assert_eq!(code, "provisioning_timeout");
    assert_eq!(capability, None);
}
