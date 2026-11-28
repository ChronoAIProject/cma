//! Immutable resource-time evidence, independent of prices and delivery.
mod allocated;
mod measured;
mod measured_model;
mod measured_support;
mod quantize;
pub mod shadow;

pub use allocated::{project, MinuteFact, Projection, MAX_FACTS};
pub use measured::freeze_measured;
pub use measured_model::{
    ExpectedMinute, FrozenMeasuredMinute, MeasuredError, MeasuredMetric, MeasuredMinute,
    MeasuredQuality,
};
pub use measured_support::ObservedSupport;

#[cfg(test)]
mod measured_tests;
#[cfg(test)]
mod quantize_tests;
#[cfg(test)]
mod tests;
