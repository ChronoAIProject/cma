use crate::ingress_budget::{sizing, AdmissionRefusal, AllocationPlan, IngressOwnershipPolicy};

pub(super) fn storage_plan() -> Result<AllocationPlan, AdmissionRefusal> {
    // The process OnceLocks retain raw, JSON tree/scratch, permission, sign-in
    // and observation pools. Their token caps are not eager payload buffers.
    AllocationPlan::checked(
        "conversation process pools",
        &[sizing::semaphore_storage_bytes()?; 6],
    )
}

pub(in crate::hosted::acp) fn require(
    policy: &IngressOwnershipPolicy,
) -> Result<(), AdmissionRefusal> {
    if policy.bounds().process_storage_bytes() < storage_plan()?.requested_bytes() {
        return Err(AdmissionRefusal::InvalidPlan {
            phase: "conversation process metadata",
        });
    }
    Ok(())
}
