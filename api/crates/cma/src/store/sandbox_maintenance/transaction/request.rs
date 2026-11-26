use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{
    RuntimeMigrationAdmissionBasis, SandboxCustomRuntimeMigrationAdmission,
    SandboxImageRotationRequest, SandboxInactivityPauseRequest,
};
use layer2_core::sandbox::{RetainedRuntimeTransition, RetainedRuntimeTransitionRequest};

pub(in crate::store::sandbox_maintenance) enum MaintenanceRequest {
    Inactivity(SandboxInactivityPauseRequest),
    Rotation(SandboxImageRotationRequest),
    FleetRotation(
        SandboxImageRotationRequest,
        Box<RuntimeMigrationAdmissionBasis>,
    ),
    Custom(Box<SandboxCustomRuntimeMigrationAdmission>),
    RetainedTransition {
        request: Box<RetainedRuntimeTransitionRequest>,
        requested_at: String,
    },
    RetainedTransitionAbort {
        receipt: Box<RetainedRuntimeTransition>,
        requested_at: String,
    },
}

impl MaintenanceRequest {
    pub(in crate::store::sandbox_maintenance) fn requires_desktop_guard(&self) -> bool {
        matches!(self, Self::FleetRotation(..) | Self::Custom(_))
    }
    pub(in crate::store::sandbox_maintenance) fn basis(
        &self,
    ) -> Option<&RuntimeMigrationAdmissionBasis> {
        match self {
            Self::FleetRotation(_, basis) => Some(basis),
            Self::Custom(admission) => Some(&admission.basis),
            _ => None,
        }
    }

    pub(in crate::store::sandbox_maintenance) fn sandbox_id(&self) -> &SandboxId {
        match self {
            Self::Inactivity(request) => &request.sandbox_id,
            Self::Rotation(request) | Self::FleetRotation(request, _) => &request.sandbox_id,
            Self::Custom(admission) => admission.sandbox_id(),
            Self::RetainedTransition { request, .. } => &request.sandbox_id,
            Self::RetainedTransitionAbort { receipt, .. } => &receipt.request.sandbox_id,
        }
    }

    pub(in crate::store::sandbox_maintenance) fn requested_at(&self) -> &str {
        match self {
            Self::Inactivity(request) => &request.requested_at,
            Self::Rotation(request) | Self::FleetRotation(request, _) => &request.requested_at,
            Self::Custom(admission) => &admission.basis.admitted_at,
            Self::RetainedTransition { requested_at, .. }
            | Self::RetainedTransitionAbort { requested_at, .. } => requested_at,
        }
    }
}
