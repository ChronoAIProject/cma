use super::*;
use layer2_core::bridge::{invoke::BridgeInvokeError, BridgeError, BridgeSetupBlock};

#[test]
fn transient_authority_failure_does_not_become_a_thirty_second_suspension() {
    for error in [BridgeError::Unavailable, BridgeError::AuthorityUnavailable] {
        assert_eq!(bridge_failure("fixture", error.clone()), Error::Unavailable);
        assert_eq!(
            invoke_failure("fixture", BridgeInvokeError::Bridge(error)),
            Error::Unavailable
        );
    }
    assert_eq!(
        invoke_failure("fixture", BridgeInvokeError::Unavailable).retry_delay(),
        Duration::from_secs(1)
    );
    assert_eq!(
        invoke_failure("fixture", BridgeInvokeError::Conflict).retry_delay(),
        Duration::from_millis(250)
    );
    assert_eq!(
        bridge_failure("fixture", BridgeError::Conflict),
        Error::Changed
    );
}

#[test]
fn revoked_suspended_and_denied_authority_keep_the_long_retry_window() {
    for error in [
        BridgeInvokeError::Revoked,
        BridgeInvokeError::Forbidden,
        BridgeInvokeError::Suspended,
        BridgeInvokeError::NotFound,
        BridgeInvokeError::TargetMismatch,
        BridgeInvokeError::Bridge(BridgeError::Forbidden),
        BridgeInvokeError::Bridge(BridgeError::AdmissionStopped),
        BridgeInvokeError::Bridge(BridgeError::ConnectionRequired(
            BridgeSetupBlock::BindingMismatch,
        )),
    ] {
        let classified = invoke_failure("fixture", error);
        assert_eq!(classified, Error::Suspended);
        assert_eq!(classified.retry_delay(), Duration::from_secs(30));
    }
}
