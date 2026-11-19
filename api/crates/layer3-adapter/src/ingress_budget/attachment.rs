//! Count complete attachment lifetimes, including independently retained owners.

use super::{
    custody::SharedCustody, AdmissionRefusal, AllocationPlan, IngressBudget, IngressClass,
    IngressOwnershipPolicy, ReservationBundle,
};
use std::sync::atomic::Ordering;

/// One startup admission. Cloning its derived policy retains this attachment;
/// opening another attachment requires another token from the process policy.
/// This governs allocation overlap only and conveys no effect authority.
pub struct IngressAttachment {
    policy: IngressOwnershipPolicy,
}

pub(super) struct AttachmentStorage {
    _storage: ReservationBundle,
    _slot: Slot,
}

struct Slot(IngressBudget);

impl IngressOwnershipPolicy {
    /// Admit before constructing the socket or starting its codec/source work.
    /// Use this token's policy for every derived allocation and consume the token
    /// exactly once at conversation open. Derived owners retain the slot even
    /// after the connection closes; sharing a root alone does not retain it.
    pub fn admit_attachment(&self) -> Result<IngressAttachment, AdmissionRefusal> {
        let maximum = self
            .bounds
            .maximum_attachments()
            .ok_or(AdmissionRefusal::InvalidPlan {
                phase: "attachment limit is not configured",
            })?;
        if self.root.3.is_some() {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "attachment admission requires the process policy",
            });
        }
        let slot = Slot::acquire(self.root.clone(), maximum)?;
        let storage = self.admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "attachment lifetime owner",
                &[
                    size_of::<AttachmentStorage>(),
                    2 * size_of::<usize>(),
                    align_of::<AttachmentStorage>(),
                ],
            )?,
        )?;
        let mut policy = self.clone();
        policy.root.3 = Some(SharedCustody::new(AttachmentStorage {
            _storage: storage,
            _slot: slot,
        }));
        Ok(IngressAttachment { policy })
    }
}

impl IngressAttachment {
    pub fn policy(&self) -> &IngressOwnershipPolicy {
        &self.policy
    }

    pub(crate) fn into_policy(self) -> IngressOwnershipPolicy {
        self.policy
    }
}

impl Slot {
    fn acquire(root: IngressBudget, maximum: usize) -> Result<Self, AdmissionRefusal> {
        root.1
            .classes
            .get()
            .expect("policy installs attachment limits")
            .active_attachments
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                active.checked_add(1).filter(|next| *next <= maximum)
            })
            .map_err(|_| AdmissionRefusal::AtCapacity {
                phase: "concurrent ingress attachments",
            })?;
        Ok(Self(root))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let previous = self
            .0
             .1
            .classes
            .get()
            .expect("attachment retains its process policy")
            .active_attachments
            .fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "attachment slot released exactly once");
    }
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
