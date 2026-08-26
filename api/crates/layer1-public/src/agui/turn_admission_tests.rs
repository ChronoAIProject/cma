//! Semantic run-admission fingerprint checks.

use super::*;

#[test]
fn a_run_fingerprint_is_the_action_not_its_envelope() {
    let first = RunAction::Prompt("do the work".into());
    let same = RunAction::Prompt("do the work".into());
    let changed = RunAction::Prompt("do different work".into());

    assert_eq!(run_fingerprint(&first).unwrap(), run_fingerprint(&same).unwrap());
    assert_ne!(
        run_fingerprint(&first).unwrap(),
        run_fingerprint(&changed).unwrap()
    );
}

#[test]
fn normalized_approval_decisions_are_part_of_the_fingerprint() {
    let allow = RunAction::Resume(vec![ApprovalResolution::new(
        "approval_1",
        ApprovalDecision::Allow,
    )]);
    let deny = RunAction::Resume(vec![ApprovalResolution::new(
        "approval_1",
        ApprovalDecision::Deny,
    )]);

    assert_ne!(
        run_fingerprint(&allow).unwrap(),
        run_fingerprint(&deny).unwrap()
    );
}
