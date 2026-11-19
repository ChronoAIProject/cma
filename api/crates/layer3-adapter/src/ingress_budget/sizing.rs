//! Checked requested-capacity manifests, separate from wire bytes and RSS.

use crate::{AdapterError, Result};

pub const SIZING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressClass {
    Data,
    Transformation,
    Control,
}

/// An allocation phase's complete simultaneous peak, including its owners.
/// Callers derive each term from bounded input and the actual container layout.
#[derive(Debug, Clone, Copy)]
pub struct AllocationPlan {
    phase: &'static str,
    requested_bytes: usize,
}

impl AllocationPlan {
    pub fn new(phase: &'static str, simultaneous_allocations: &[usize]) -> Result<Self> {
        let requested_bytes = simultaneous_allocations
            .iter()
            .try_fold(0usize, |sum, bytes| sum.checked_add(*bytes))
            .filter(|bytes| *bytes <= u32::MAX as usize)
            .ok_or_else(invalid)?;
        Ok(Self {
            phase,
            requested_bytes,
        })
    }

    pub fn phase(self) -> &'static str {
        self.phase
    }

    pub fn requested_bytes(self) -> usize {
        self.requested_bytes
    }
}

/// Disjoint class ceilings under one root. Data cannot borrow either floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassBounds {
    total: usize,
    control: usize,
    transformation: usize,
}

impl ClassBounds {
    pub fn new(
        total: usize,
        concurrent_control: AllocationPlan,
        largest_transformation: AllocationPlan,
    ) -> Result<Self> {
        let control = concurrent_control.requested_bytes();
        let transformation = largest_transformation.requested_bytes();
        let floors = control.checked_add(transformation).ok_or_else(invalid)?;
        if control == 0 || transformation == 0 || floors >= total || total > u32::MAX as usize {
            return Err(invalid());
        }
        Ok(Self {
            total,
            control,
            transformation,
        })
    }

    pub fn sizing_version(self) -> u32 {
        SIZING_VERSION
    }
    pub fn total_bytes(self) -> usize {
        self.total
    }
    pub fn data_bytes(self) -> usize {
        self.total - self.control - self.transformation
    }
    pub fn control_bytes(self) -> usize {
        self.control
    }
    pub fn transformation_bytes(self) -> usize {
        self.transformation
    }
}

pub fn array_bytes<T>(capacity: usize) -> Result<usize> {
    capacity.checked_mul(size_of::<T>()).ok_or_else(invalid)
}

/// Both Arc headers, the payload layout and the separately shared reservation.
pub fn shared_owner_bytes<T>() -> usize {
    size_of::<T>()
        + size_of::<super::ReservationBundle>()
        + 4 * size_of::<usize>()
        + align_of::<T>()
        + align_of::<super::ReservationBundle>()
}

pub(super) fn invalid() -> AdapterError {
    AdapterError::Rejected("ingress allocation plan exceeds its supported bounds".into())
}
