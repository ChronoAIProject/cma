//! The per-session size range this deployment permits callers to state.

use std::fmt;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::size::SessionSize;

/// Which dimension of a size a refusal is about.
///
/// The name carries the unit, so nothing downstream has to be told one
/// separately and no wire field can disagree with the number beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeDimension {
    /// Processor time, in thousandths of one core.
    CpuMillis,
    /// Working memory, in mebibytes.
    MemoryMib,
}

impl SizeDimension {
    /// The wire name, which is what a refusal quotes. Part of the contract.
    pub fn name(self) -> &'static str {
        match self {
            Self::CpuMillis => "cpu_millis",
            Self::MemoryMib => "memory_mib",
        }
    }
}

impl fmt::Display for SizeDimension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// What a session on this deployment may ask for.
///
/// These bounds govern only a size the caller stated. A session that asks for
/// no size is provisioned at the deployment's own reservation and is not
/// measured here. If a later resolver makes that reservation numeric, it must
/// preserve whether the caller stated a size rather than applying this check to
/// the resolved default: changing bounds must not make an unnamed-size profile
/// unstartable.
///
/// Bounds are a per-request range. They say nothing about whether space exists
/// across the deployment. A request can be inside this range and still be
/// unschedulable because other sessions have consumed the available capacity;
/// a deployment-wide capacity ledger is a separate decision and type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CapacityBounds {
    /// Smallest processor request accepted, in thousandths of one core.
    #[schema(value_type = u32, minimum = 1)]
    pub min_cpu_millis: NonZeroU32,
    /// Largest processor request accepted, in thousandths of one core.
    #[schema(value_type = u32, minimum = 1)]
    pub max_cpu_millis: NonZeroU32,
    /// Smallest working-memory request accepted, in mebibytes.
    #[schema(value_type = u32, minimum = 1)]
    pub min_memory_mib: NonZeroU32,
    /// Largest working-memory request accepted, in mebibytes.
    #[schema(value_type = u32, minimum = 1)]
    pub max_memory_mib: NonZeroU32,
}

/// A size this deployment will not provision, and the bound that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{dimension}: asked for {requested}, this deployment permits {minimum} to {maximum}")]
pub struct OutOfBounds {
    /// Which figure was outside the range.
    pub dimension: SizeDimension,
    /// The figure the caller stated.
    pub requested: u32,
    /// The inclusive lower bound.
    pub minimum: u32,
    /// The inclusive upper bound.
    pub maximum: u32,
}

impl CapacityBounds {
    /// Fail unless this size is one this deployment will provision.
    ///
    /// Processor first, then working memory, so the answer is stable for a size
    /// that is wrong in both: a refusal that names a different dimension on
    /// each attempt reads as a service changing its mind.
    pub fn check(self, size: SessionSize) -> Result<(), OutOfBounds> {
        let requested = size.cpu_millis();
        let minimum = self.min_cpu_millis.get();
        let maximum = self.max_cpu_millis.get();
        if !(minimum..=maximum).contains(&requested) {
            return Err(OutOfBounds {
                dimension: SizeDimension::CpuMillis,
                requested,
                minimum,
                maximum,
            });
        }

        let requested = size.memory_mib();
        let minimum = self.min_memory_mib.get();
        let maximum = self.max_memory_mib.get();
        if !(minimum..=maximum).contains(&requested) {
            return Err(OutOfBounds {
                dimension: SizeDimension::MemoryMib,
                requested,
                minimum,
                maximum,
            });
        }

        Ok(())
    }

    /// Whether a size sits inside these bounds. [`Self::check`] without the reason.
    pub fn contains(self, size: SessionSize) -> bool {
        self.check(size).is_ok()
    }
}

impl Default for CapacityBounds {
    /// The shipped range when an operator has decided nothing.
    ///
    /// It contains the deployment's existing reservation of 1000 processor
    /// thousandths and 1024 mebibytes, so stating the size callers already get
    /// is not a refusal. Each maximum is one fifth of the deployment quota, so
    /// one session cannot consume the whole deployment by default. Each minimum
    /// refuses a runtime too small to do useful work before it is provisioned.
    fn default() -> Self {
        Self {
            min_cpu_millis: NonZeroU32::new(250).expect("the shipped minimum is non-zero"),
            max_cpu_millis: NonZeroU32::new(4_000).expect("the shipped maximum is non-zero"),
            min_memory_mib: NonZeroU32::new(256).expect("the shipped minimum is non-zero"),
            max_memory_mib: NonZeroU32::new(8_192).expect("the shipped maximum is non-zero"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cpu_millis: u32, memory_mib: u32) -> SessionSize {
        SessionSize::new(cpu_millis, memory_mib).expect("test sizes are non-zero")
    }

    #[test]
    fn the_shipped_bounds_contain_what_this_deployment_already_reserves() {
        // Literal on purpose. The deployment currently reserves these figures,
        // but its configuration is opaque quantity text and Layer 2 must not
        // parse or import it merely to establish its own policy default.
        assert!(CapacityBounds::default().contains(size(1_000, 1_024)));
    }

    #[test]
    fn processor_is_reported_first_when_both_figures_are_wrong() {
        let refusal = CapacityBounds::default()
            .check(size(4_001, 8_193))
            .expect_err("both figures are above their maxima");
        assert_eq!(refusal.dimension, SizeDimension::CpuMillis);
    }

    #[test]
    fn every_side_of_each_range_reports_its_exact_bound() {
        let bounds = CapacityBounds::default();
        for (asked, expected) in [
            (
                size(249, 1_024),
                OutOfBounds {
                    dimension: SizeDimension::CpuMillis,
                    requested: 249,
                    minimum: 250,
                    maximum: 4_000,
                },
            ),
            (
                size(4_001, 1_024),
                OutOfBounds {
                    dimension: SizeDimension::CpuMillis,
                    requested: 4_001,
                    minimum: 250,
                    maximum: 4_000,
                },
            ),
            (
                size(1_000, 255),
                OutOfBounds {
                    dimension: SizeDimension::MemoryMib,
                    requested: 255,
                    minimum: 256,
                    maximum: 8_192,
                },
            ),
            (
                size(1_000, 8_193),
                OutOfBounds {
                    dimension: SizeDimension::MemoryMib,
                    requested: 8_193,
                    minimum: 256,
                    maximum: 8_192,
                },
            ),
        ] {
            assert_eq!(bounds.check(asked), Err(expected));
        }
    }

    #[test]
    fn both_inclusive_edges_are_accepted() {
        let bounds = CapacityBounds::default();
        assert!(bounds.check(size(250, 256)).is_ok());
        assert!(bounds.check(size(4_000, 8_192)).is_ok());
    }

    #[test]
    fn dimension_names_carry_their_units() {
        assert_eq!(SizeDimension::CpuMillis.name(), "cpu_millis");
        assert_eq!(SizeDimension::MemoryMib.name(), "memory_mib");
    }
}
