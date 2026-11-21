//! Shared message validation and unchanged tool error mapping.
use cmaeg_core::{
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};

pub(in crate::tools) fn validate(message: &OutboundMessage) -> Result<(), Problem> {
    message.validate().map_err(|error| {
        Problem::new(
            if error.message.contains("32768") {
                ProblemCode::PayloadTooLarge
            } else {
                ProblemCode::InvalidRequest
            },
            if error.message.contains("32768") {
                413
            } else {
                400
            },
            error.to_string(),
        )
    })
}
