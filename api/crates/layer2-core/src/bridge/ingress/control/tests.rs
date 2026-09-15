use super::{classify, ChannelControlRefusal as R, MAX_CONTROL_BYTES};

#[test]
fn reserved_controls_cannot_become_messages_when_disabled_or_malformed() {
    for (text, expected) in [
        (
            "CMA-CONTROL/1 cancel ccr_original",
            R::ChannelControlsDisabled,
        ),
        (
            "CMA-CONTROL/1 steer ccr_original\nCheck the migration first.",
            R::ChannelControlsDisabled,
        ),
        (
            "CMA-CONTROL/2 cancel ccr_original",
            R::ChannelControlVersionUnsupported,
        ),
        (
            "CMA-CONTROL/1 approve ccr_original",
            R::ApprovalDashboardOnly,
        ),
        ("CMA-CONTROL/1 deny", R::ApprovalDashboardOnly),
        (
            "CMA-CONTROL/1 reset ccr_original",
            R::ChannelControlOperationUnsupported,
        ),
        ("CMA-CONTROL", R::InvalidChannelControlRequest),
        (
            "CMA-CONTROL/1  cancel ccr_original",
            R::InvalidChannelControlRequest,
        ),
        (
            " CMA-CONTROL/1 cancel ccr_original",
            R::InvalidChannelControlRequest,
        ),
        (
            "CMA-CONTROL/1 cancel ccr_original\n",
            R::InvalidChannelControlRequest,
        ),
        (
            "CMA-CONTROL/1 cancel ccr_original extra",
            R::InvalidChannelControlRequest,
        ),
        ("CMA-CONTROL/1 cancel ccr_", R::InvalidChannelControlRequest),
        (
            "CMA-CONTROL/1 steer ccr_original\n \t",
            R::InvalidChannelControlRequest,
        ),
    ] {
        assert_eq!(classify(text), Some(expected), "{text}");
    }
    let oversized = format!(
        "CMA-CONTROL/1 steer ccr_original\n{}",
        "x".repeat(MAX_CONTROL_BYTES)
    );
    assert_eq!(classify(&oversized), Some(R::InvalidChannelControlRequest));
}

#[test]
fn ordinary_chat_and_quoted_examples_have_no_control_interpretation() {
    for text in [
        "yes",
        "approve",
        "cancel",
        "Please cancel that",
        "Example: CMA-CONTROL/1 cancel ccr_x",
        "cma-control/1 cancel ccr_x",
    ] {
        assert_eq!(classify(text), None);
    }
}
