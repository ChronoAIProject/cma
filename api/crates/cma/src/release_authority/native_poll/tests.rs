use super::super::{tests::head, AuthorityError};
use super::{model::*, records, testing, *};

mod mongo;

#[test]
fn opaque_command_identity_cannot_change_producer_sequence_or_predecessor() {
    let (command, _) = testing::fixture(NativePollLane::Background, None);
    let mut changed = command.0.clone();
    changed.lane = NativePollLane::StartupAgent;
    assert_eq!(changed.validate(), Err(AuthorityError::Invalid));
    changed = command.0.clone();
    changed.sequence += 1;
    assert_eq!(changed.validate(), Err(AuthorityError::Invalid));
    changed = command.0.clone();
    changed.writer.instance_id.push_str("-foreign");
    assert_eq!(changed.validate(), Err(AuthorityError::Invalid));
    changed = command.0;
    changed.previous_receipt_sha256 = Some("b".repeat(64));
    assert_eq!(changed.validate(), Err(AuthorityError::Invalid));
}

#[test]
fn next_sequence_requires_the_exact_observed_receipt() {
    let (_, receipt) = testing::fixture(NativePollLane::Background, None);
    assert_eq!(records::next_sequence(None, None), Ok(1));
    assert_eq!(
        records::next_sequence(Some(&receipt), None),
        Err(AuthorityError::Conflict)
    );
    assert_eq!(
        records::next_sequence(None, Some(&receipt)),
        Err(AuthorityError::Conflict)
    );
    assert_eq!(
        records::next_sequence(Some(&receipt), Some(&receipt)),
        Ok(2)
    );
    let (_, other_lane) = testing::fixture(NativePollLane::StartupAgent, None);
    assert_eq!(
        records::next_sequence(Some(&receipt), Some(&other_lane)),
        Err(AuthorityError::Conflict)
    );
}

#[test]
fn immutable_receipt_rejects_changed_result_or_lease() {
    let (_, receipt) = testing::fixture(NativePollLane::Background, Some(testing::target()));
    let mut changed = receipt.0.clone();
    changed.outcome = NativePollOutcome::Empty;
    assert_eq!(changed.validate(), Err(AuthorityError::Invalid));
    let mut changed = receipt.0;
    let NativePollOutcome::Claimed(target) = &mut changed.outcome else {
        panic!("positive fixture")
    };
    target.lease = Some("other-lease".into());
    changed.receipt_sha256 = changed.digest().unwrap();
    assert_eq!(changed.validate(), Err(AuthorityError::Invalid));
}

#[test]
fn target_validation_rejects_malformed_and_wrong_family_results() {
    let (command, receipt) = testing::fixture(NativePollLane::Background, Some(testing::target()));
    let NativePollOutcome::Claimed(target) = receipt.outcome() else {
        panic!("positive fixture")
    };
    for field in 0..4 {
        let mut target = (**target).clone();
        match field {
            0 => target.applied_revision = target.revision + 1,
            1 => target.document_schema_version = 1,
            2 => target.plan.deployment = "foreign".into(),
            _ => target.machine = true,
        }
        assert_eq!(
            validate_target(&command.0, &target),
            Err(AuthorityError::Invalid)
        );
    }
}

#[test]
fn old_head_and_inactive_or_foreign_writer_never_admit_poll() {
    let mut h = head();
    let writer = h.writer.clone().unwrap();
    assert!(active_writer(&h, &writer).is_ok());
    h.schema_version = 1;
    assert_eq!(active_writer(&h, &writer), Err(AuthorityError::Invalid));
    h = head();
    let mut foreign = writer.clone();
    foreign.instance_id.push_str("-foreign");
    assert_eq!(active_writer(&h, &foreign), Err(AuthorityError::Closed));
    h.phase = Phase::Freezing;
    h.operation_id = Some("freeze".into());
    assert_eq!(active_writer(&h, &writer), Err(AuthorityError::Closed));
}

#[test]
fn startup_lanes_bind_only_current_pair_and_machine_must_exist() {
    let mut h = head();
    assert_eq!(
        selected(&h, NativePollLane::StartupAgent).unwrap(),
        Some(h.pair.agent.clone())
    );
    assert_eq!(
        selected(&h, NativePollLane::StartupMachine).unwrap(),
        h.pair.machine
    );
    h.pair.machine = None;
    assert_eq!(
        selected(&h, NativePollLane::StartupMachine),
        Err(AuthorityError::Closed)
    );
}

#[test]
fn startup_receipt_validates_original_binding_and_exact_definition() {
    let (command, receipt) =
        testing::fixture(NativePollLane::StartupAgent, Some(testing::bound_target()));
    let NativePollOutcome::Claimed(target) = receipt.outcome() else {
        panic!("positive fixture")
    };
    let mut replaced = (**target).clone();
    replaced
        .binding
        .as_mut()
        .unwrap()
        .uid
        .push_str("-replacement");
    assert_eq!(
        validate_target(&command.0, &replaced),
        Err(AuthorityError::Invalid)
    );
    let mut changed = (**target).clone();
    changed.plan.spec.image.uri = format!("registry.example/agent@sha256:{}", "f".repeat(64));
    assert_eq!(
        validate_target(&command.0, &changed),
        Err(AuthorityError::Invalid)
    );
}
