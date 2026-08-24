//! How large a session's runtime is, in units this service owns.
//!
//! A `SessionSize` never represents the deployment's own reservation. That
//! reservation remains a pair of opaque quantities in deployment configuration,
//! and `Option<SessionSize>::None` is its only representation in the per-session
//! path. Consequently, bounds apply only to a size a caller stated and skip
//! `None`; a future capacity ledger that needs a numeric deployment reservation
//! must take an administrator-stated value rather than derive one here.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// How much a session's runtime reserves.
///
/// Both figures are non-zero by construction, for the reason a terminal
/// geometry's are (`crate::terminal`): a zero here describes a runtime that
/// cannot be scheduled, and making it unrepresentable means no downstream code
/// has to re-validate it - least of all the provider, which would discover it
/// minutes later with nobody left to tell.
///
/// The units are this service's own and are named in the fields. Nothing here
/// carries a provider's quantity syntax: the number is canonical and the string
/// is derived once in the layer that owns the provider's words.
///
/// Only zero is rejected here. Minimum and maximum sizes are policy an
/// administrator sets at runtime; hard-coding either would create a second
/// bounds mechanism that the administrator cannot change.
// Deliberately no `Default`: `None` means the deployment's own reservation, and
// a default value here would let a later `unwrap_or_default()` invent a size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionSize {
    /// Processor time, in thousandths of one core.
    #[schema(value_type = u32, minimum = 1)]
    pub cpu_millis: NonZeroU32,
    /// Working memory, in mebibytes.
    #[schema(value_type = u32, minimum = 1)]
    pub memory_mib: NonZeroU32,
}

impl SessionSize {
    /// Build a size, rejecting a zero in either figure.
    pub fn new(cpu_millis: u32, memory_mib: u32) -> Result<Self, InvalidSessionSize> {
        match (NonZeroU32::new(cpu_millis), NonZeroU32::new(memory_mib)) {
            (Some(cpu_millis), Some(memory_mib)) => Ok(Self {
                cpu_millis,
                memory_mib,
            }),
            _ => Err(InvalidSessionSize {
                cpu_millis,
                memory_mib,
            }),
        }
    }

    /// Processor time as a plain integer.
    pub fn cpu_millis(self) -> u32 {
        self.cpu_millis.get()
    }

    /// Working memory as a plain integer.
    pub fn memory_mib(self) -> u32 {
        self.memory_mib.get()
    }
}

/// A size was rejected because a figure was zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "a session size must be non-zero in both figures, got {cpu_millis}m CPU and {memory_mib}MiB"
)]
pub struct InvalidSessionSize {
    /// The offending processor figure.
    pub cpu_millis: u32,
    /// The offending memory figure.
    pub memory_mib: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_size_is_accepted() {
        let size = SessionSize::new(1_500, 3_072).expect("a non-zero size is usable");
        assert_eq!(size.cpu_millis(), 1_500);
        assert_eq!(size.memory_mib(), 3_072);
    }

    #[test]
    fn a_zero_in_either_figure_is_rejected() {
        for (cpu_millis, memory_mib) in [(0, 512), (500, 0), (0, 0)] {
            assert!(
                SessionSize::new(cpu_millis, memory_mib).is_err(),
                "{cpu_millis}m CPU and {memory_mib}MiB must fail"
            );
        }
    }

    #[test]
    fn a_rejection_reports_the_offending_figures() {
        let error = SessionSize::new(0, 512).expect_err("zero CPU must fail");
        assert_eq!(error.cpu_millis, 0);
        assert_eq!(error.memory_mib, 512);
    }

    #[test]
    fn a_size_survives_its_own_wire_form() {
        let size = SessionSize::new(750, 2_048).expect("a non-zero size is usable");
        let json = serde_json::to_value(size).expect("a size serializes");
        assert_eq!(json["cpu_millis"], 750);
        assert_eq!(json["memory_mib"], 2_048);
        assert_eq!(
            serde_json::from_value::<SessionSize>(json).expect("a size deserializes"),
            size
        );
    }

    #[test]
    fn a_stored_zero_is_refused_rather_than_read_back() {
        let result = serde_json::from_str::<SessionSize>(r#"{"cpu_millis":0,"memory_mib":512}"#);
        assert!(
            result.is_err(),
            "a deserialized zero must stay unrepresentable"
        );
    }
}
