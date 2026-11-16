use super::*;
use layer2_core::budget::{Closure, ClosureReason};

fn state(reasons: &[ClosureReason]) -> PlatformAdmissionState {
    PlatformAdmissionState {
        configured: true,
        operator_control: layer2_core::budget::OperatorAdmissionState::Open,
        closures: reasons
            .iter()
            .map(|reason| Closure {
                reason: *reason,
                since: 1,
                settings_revision: 1,
                committed_at_entry: 0,
            })
            .collect(),
    }
}

#[test]
fn registration_is_closed_for_new_subjects_only_while_a_closure_exists_and_existing_owners_pass() {
    use ClosureReason::*;
    use RegistrationDecision::*;
    for (closures, existing, expected) in [
        (&[][..], false, Admitted),
        (&[][..], true, Existing),
        (&[SpendWarning][..], false, Closed),
        (&[SpendWarning][..], true, Existing),
        (&[SpendStop][..], false, Closed),
        (&[SpendStop, SpendWarning][..], true, Existing),
        (&[MeteringUnavailable][..], false, Closed),
    ] {
        assert_eq!(
            registration_decision(&state(closures), existing),
            expected,
            "{closures:?} existing={existing}"
        );
    }
    assert_eq!(
        registration_decision(&PlatformAdmissionState::default(), false),
        Admitted
    );
}

#[test]
fn operator_stop_closes_registration_without_changing_existing_owner_sign_in() {
    let stopped = PlatformAdmissionState {
        operator_control: layer2_core::budget::OperatorAdmissionState::Stopped(
            layer2_core::admission::OperatorStop {
                since: 1,
                revision: 1,
                actor: SubjectId::new("operator"),
                reason: "contain".into(),
            },
        ),
        ..Default::default()
    };
    assert_eq!(
        registration_decision(&stopped, false),
        RegistrationDecision::Closed
    );
    assert_eq!(
        registration_decision(&stopped, true),
        RegistrationDecision::Existing
    );
}

#[test]
fn unreadable_control_closes_new_registration_but_preserves_existing_owners() {
    let unreadable = PlatformAdmissionState {
        operator_control: layer2_core::budget::OperatorAdmissionState::Unreadable,
        ..Default::default()
    };
    assert_eq!(
        registration_decision(&unreadable, false),
        RegistrationDecision::Closed
    );
    assert_eq!(
        registration_decision(&unreadable, true),
        RegistrationDecision::Existing
    );
}
