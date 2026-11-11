use super::*;
use layer2_core::budget::{Closure, ClosureReason};

fn state(reasons: &[ClosureReason]) -> PlatformAdmissionState {
    PlatformAdmissionState {
        configured: true,
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
fn registration_is_closed_for_new_subjects_only_while_a_closure_exists() {
    use ClosureReason::*;
    use RegistrationDecision::*;
    for (closures, existing, expected) in [
        (&[][..], false, Admitted),
        (&[][..], true, Admitted),
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
