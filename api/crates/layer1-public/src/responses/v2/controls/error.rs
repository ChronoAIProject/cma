use axum::{http::StatusCode, response::Response};
use layer2_core::sandbox::agent_io::{
    CommandAdmissionError, CommandLedgerError, TargetedControlError,
};

use super::super::error;

pub(super) fn control(value: TargetedControlError) -> Response {
    match value {
        TargetedControlError::CapabilityUnsupported => error::field(
            "command.type",
            "capability_unsupported",
            "The observed Agent runtime does not support this control operation.",
        ),
        TargetedControlError::NotAdmitted => error::error(
            StatusCode::CONFLICT,
            "response_control_not_admitted",
            "The original control was not admitted and its Agent revision is permanently stale. Observe again before creating a new intent.",
            None,
        ),
        TargetedControlError::Admission(value) => admission(value),
    }
}

pub(in crate::responses::v2) fn admission(value: CommandAdmissionError) -> Response {
    match value {
        value @ CommandAdmissionError::Input(
            layer2_core::sandbox::AgentOperationError::ModelFundingUnsupported
            | layer2_core::sandbox::AgentOperationError::ModelFunding(_),
        ) => error::admission(value),
        CommandAdmissionError::Ledger(CommandLedgerError::Conflict | CommandLedgerError::NotAdmissible | CommandLedgerError::StaleClaim) => {
            error::error(StatusCode::CONFLICT, "response_control_conflict",
                "The observed Agent, response turn or pending approval cannot accept this control. Recover an uncertain admission with the original key and body; observe again before creating a new intent.", None)
        }
        CommandAdmissionError::Ledger(CommandLedgerError::Invalid) | CommandAdmissionError::Input(_) => {
            error::field("command", "invalid_control_request", "The response control request is invalid.")
        }
        other => error::admission(other),
    }
}

#[cfg(test)]
#[tokio::test]
async fn control_model_funding_contract() {
    crate::model_funding_problem_tests::assert_funding_contract(
        |value| control(TargetedControlError::Admission(value.into())),
        true,
    )
    .await;
}

#[cfg(test)]
#[tokio::test]
async fn control_invalid_input_remains_invalid() {
    crate::model_funding_problem_tests::assert_invalid_contract(
        |value| control(TargetedControlError::Admission(value.into())),
        "invalid_control_request",
    )
    .await;
}
