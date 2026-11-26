//! Checked requested-capacity manifests, separate from wire bytes and RSS.

use crate::{AdapterError, Result};
use std::alloc::{Layout, LayoutError};
use std::num::NonZeroUsize;

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
    metadata: usize,
    maximum_attachments: Option<NonZeroUsize>,
}

impl ClassBounds {
    pub fn new(
        total: usize,
        concurrent_control: AllocationPlan,
        largest_transformation: AllocationPlan,
    ) -> Result<Self> {
        let control = concurrent_control.requested_bytes();
        let transformation = largest_transformation.requested_bytes();
        let metadata = AllocationPlan::new(
            "resident root and class metadata",
            &[
                super::classes::storage_bytes().map_err(AdapterError::from)?,
                super::root_storage::Bootstrap::new().bytes(),
            ],
        )?
        .requested_bytes();
        let floors = control
            .checked_add(transformation)
            .and_then(|bytes| bytes.checked_add(metadata))
            .ok_or_else(invalid)?;
        if control == 0 || transformation == 0 || floors >= total || total > u32::MAX as usize {
            return Err(invalid());
        }
        Ok(Self {
            total,
            control,
            transformation,
            metadata,
            maximum_attachments: None,
        })
    }

    pub fn sizing_version(self) -> u32 {
        SIZING_VERSION
    }
    pub fn total_bytes(self) -> usize {
        self.total
    }
    pub fn data_bytes(self) -> usize {
        self.total - self.control - self.transformation - self.metadata
    }
    pub fn control_bytes(self) -> usize {
        self.control
    }
    pub fn transformation_bytes(self) -> usize {
        self.transformation
    }

    pub fn metadata_bytes(self) -> usize {
        self.metadata
    }

    pub(crate) fn process_storage_bytes(self) -> usize {
        self.metadata
            - super::root_storage::Bootstrap::new().bytes()
            - super::classes::storage_bytes().expect("bounds validate the platform layout")
    }

    /// Process-static allocations precede admitted load and outlive attachments.
    pub(crate) fn with_process_storage(mut self, plan: AllocationPlan) -> Result<Self> {
        let metadata = self
            .metadata
            .checked_add(plan.requested_bytes())
            .ok_or_else(invalid)?;
        let reserved = metadata
            .checked_add(self.control)
            .and_then(|bytes| bytes.checked_add(self.transformation))
            .ok_or_else(invalid)?;
        if reserved >= self.total {
            return Err(invalid());
        }
        self.metadata = metadata;
        Ok(self)
    }

    pub fn maximum_attachments(self) -> Option<usize> {
        self.maximum_attachments.map(NonZeroUsize::get)
    }

    pub(crate) fn with_attachment_limit(mut self, maximum: usize) -> Result<Self> {
        self.maximum_attachments = Some(NonZeroUsize::new(maximum).ok_or_else(invalid)?);
        Ok(self)
    }
}

pub fn array_bytes<T>(capacity: usize) -> Result<usize> {
    capacity.checked_mul(size_of::<T>()).ok_or_else(invalid)
}

pub(crate) fn semaphore_storage_bytes() -> std::result::Result<usize, super::AdmissionRefusal> {
    if !cfg!(any(target_os = "linux", target_vendor = "apple")) {
        return Err(super::AdmissionRefusal::InvalidPlan {
            phase: "unsupported class pool layout",
        });
    }
    // Tokio 1.53.3 releases permits through the pinned std mutex.
    AllocationPlan::checked(
        "semaphore storage",
        &[
            size_of::<tokio::sync::Semaphore>(),
            2 * size_of::<usize>(),
            align_of::<tokio::sync::Semaphore>(),
            super::root_storage::lazy_lock_bytes(),
        ],
    )
    .map(AllocationPlan::requested_bytes)
}

/// Both Arc headers, the payload layout and the separately shared reservation.
pub fn shared_owner_bytes<T>() -> usize {
    shared_allocation_bytes::<T>() + shared_allocation_bytes::<super::ReservationBundle>()
}

pub(crate) fn shared_allocation_bytes<T>() -> usize {
    shared_allocation_layout(Layout::new::<T>())
        .expect("supported shared allocation layout")
        .size()
}

fn shared_allocation_layout(payload: Layout) -> std::result::Result<Layout, LayoutError> {
    // Rust 1.96.1 ArcInner is repr(C, align(2)). Layout arithmetic avoids
    // instantiating an oversized wrapper type when only its size is requested.
    let header = Layout::new::<[std::sync::atomic::AtomicUsize; 2]>().align_to(2)?;
    let (combined, _) = header.extend(payload)?;
    Ok(combined.pad_to_align())
}

/// Fixed payload and component-custody Arc layouts, including headers/padding.
/// Dynamic payload buffers and initialization scratch must be added separately.
pub fn component_shared_owner_bytes<T>() -> std::result::Result<usize, super::AdmissionRefusal> {
    let invalid = |_| super::AdmissionRefusal::InvalidPlan {
        phase: "component shared owner layout",
    };
    AllocationPlan::checked(
        "component shared owner layout",
        &[
            shared_allocation_layout(Layout::new::<T>())
                .map_err(invalid)?
                .size(),
            shared_allocation_layout(Layout::new::<super::IngressReservation>())
                .map_err(invalid)?
                .size(),
        ],
    )
    .map(AllocationPlan::requested_bytes)
}

/// Fallible initialization constructs exactly one returned owner. Its plan must
/// also include both branches' payload capacities and simultaneous scratch.
pub fn fallible_shared_owner_bytes<T, E>() -> usize {
    shared_owner_bytes::<T>().max(shared_owner_bytes::<E>())
}

/// Tokio 1.53's bounded MPSC uses 32 slots on 64-bit, 16 on 32-bit targets.
/// Include both boundary blocks and a speculative block per concurrent sender.
/// The channel Arc has two cache-padded fields (at most 128 bytes each), Notify,
/// the bounded semaphore, six pointer-sized fields and alignment padding.
pub fn channel_bytes<T>(capacity: usize, concurrent_senders: usize) -> Result<usize> {
    channel_bytes_checked::<T>(capacity, concurrent_senders).map_err(Into::into)
}

pub fn channel_bytes_checked<T>(
    capacity: usize,
    concurrent_senders: usize,
) -> std::result::Result<usize, super::AdmissionRefusal> {
    let invalid = || super::AdmissionRefusal::InvalidPlan {
        phase: "bounded channel layout",
    };
    let slots = if usize::BITS == 64 { 32 } else { 16 };
    let blocks = capacity
        .div_ceil(slots)
        .checked_add(2)
        .and_then(|n| n.checked_add(concurrent_senders))
        .ok_or_else(invalid)?;
    let block = slots
        .checked_mul(size_of::<T>())
        .ok_or_else(invalid)?
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
    super::diagnostic::make(
        AdapterError::Rejected,
        format_args!("ingress allocation plan exceeds its supported bounds"),
    )
}

#[cfg(test)]
#[path = "sizing_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "process_storage_tests.rs"]
mod process_storage_tests;
