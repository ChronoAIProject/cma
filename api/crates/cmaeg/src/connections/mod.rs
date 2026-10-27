//! Gateway-owned platform setup, independent of every agent provider's workflow.
pub(crate) mod binding;
#[cfg(test)]
mod capability_tests;
mod completion;
#[cfg(test)]
mod consumer_tests;
pub mod model;
mod platform;
#[cfg(test)]
mod platform_tests;
mod recovery;
mod retry;
pub(crate) mod service;
pub mod store;
#[cfg(test)]
mod tests;
mod webhook;
pub use service::ConnectionService;

#[derive(Debug)]
pub enum Error {
    NotFound,
    Invalid(&'static str),
    /// Refused by name, such as `consumer_not_supported`.
    Forbidden(&'static str),
    Conflict,
    Unavailable,
}
impl Error {
    pub fn problem(self) -> cmaeg_core::problem::Problem {
        use cmaeg_core::problem::{Problem, ProblemCode};
        let (code, status, detail) = match self {
            Self::NotFound => (ProblemCode::ChannelNotFound, 404, "connection not found"),
            Self::Invalid(message) => (ProblemCode::InvalidRequest, 400, message),
            Self::Forbidden(detail) => (ProblemCode::Forbidden, 403, detail),
            Self::Conflict => (
                ProblemCode::Conflict,
                409,
                "connection changed or is already in use",
            ),
            Self::Unavailable => (
                ProblemCode::ProviderUnavailable,
                503,
                "connection setup is temporarily unavailable",
            ),
        };
        Problem::new(code, status, detail)
    }
}
