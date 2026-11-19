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
        Self::checked(phase, simultaneous_allocations).map_err(Into::into)
    }

    pub fn checked(
        phase: &'static str,
        simultaneous_allocations: &[usize],
    ) -> std::result::Result<Self, super::AdmissionRefusal> {
        let requested_bytes = simultaneous_allocations
            .iter()
            .try_fold(0usize, |sum, bytes| sum.checked_add(*bytes))
            .filter(|bytes| *bytes <= u32::MAX as usize)
            .ok_or(super::AdmissionRefusal::InvalidPlan { phase })?;
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

/// Tokio 1.53's bounded MPSC uses 32 slots on 64-bit, 16 on 32-bit targets.
/// Include both boundary blocks and a speculative block per concurrent sender.
/// The channel Arc has two cache-padded fields (at most 128 bytes each), Notify,
/// the bounded semaphore, six pointer-sized fields and alignment padding.
pub fn channel_bytes<T>(capacity: usize, concurrent_senders: usize) -> Result<usize> {
    let slots = if usize::BITS == 64 { 32 } else { 16 };
    let blocks = capacity
        .div_ceil(slots)
        .checked_add(2)
        .and_then(|n| n.checked_add(concurrent_senders))
        .ok_or_else(invalid)?;
    let block = array_bytes::<T>(slots)?
        .checked_add(4 * size_of::<usize>())
        .and_then(|n| n.checked_add(align_of::<T>()))
        .ok_or_else(invalid)?;
    let channel = 3 * 128
        + size_of::<tokio::sync::Notify>()
        + size_of::<tokio::sync::Semaphore>()
        + size_of::<[usize; 8]>();
    blocks
        .checked_mul(block)
        .and_then(|n| n.checked_add(channel))
        .ok_or_else(invalid)
}

pub(super) fn invalid() -> AdapterError {
    AdapterError::Rejected("ingress allocation plan exceeds its supported bounds".into())
}
