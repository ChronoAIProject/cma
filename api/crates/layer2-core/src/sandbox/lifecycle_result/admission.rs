use super::super::{
    control::{
        SandboxControl, SandboxControlDisposition, SandboxControlRecord, SandboxControlStatus,
    },
    Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus,
};

/// Read-only evidence reconstructed from the predecessor's aggregate and history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxLifecycleAdmission {
    pub operation_id: String,
    pub requested_at: String,
}

impl SandboxLifecycleAdmission {
    pub fn is_boundary(event: &SandboxEvent) -> bool {
        matches!(
            event.kind,
            SandboxEventKind::StatusChanged { .. }
                | SandboxEventKind::RuntimeRecoveryAdmitted { .. }
                | SandboxEventKind::Revived
                | SandboxEventKind::Rotated { .. }
        )
    }

    /// The latest three lifecycle boundaries, newest first, must come from the
    /// same snapshot as the failed aggregate. Completion invalidates old admissions.
    pub fn event<'a>(
        sandbox: &Sandbox,
        transitions: &'a [SandboxEvent],
    ) -> Option<&'a SandboxEvent> {
        if sandbox.status != SandboxStatus::Failed || sandbox.lifecycle.operation.is_some() {
            return None;
        }
        let mut events = transitions.iter();
        let mut event = events.next()?;
        let failed_from = if let SandboxEventKind::StatusChanged {
            from,
            to: SandboxStatus::Failed,
        } = event.kind
        {
            event = events.next()?;
            Some(from)
        } else {
            None
        };
        let SandboxEventKind::StatusChanged { to, .. } = event.kind else {
            return None;
        };
        if !matches!(to, SandboxStatus::Pausing | SandboxStatus::Reviving | SandboxStatus::Rotating)
            || failed_from.is_some_and(|from| from != to)
            || event.sandbox_id != sandbox.id
            || crate::seconds_from_rfc3339(&event.appended_at).is_none()
            // Runtime loss may replace authority without a status transition.
            || sandbox.lifecycle.interruption.as_ref().is_some_and(|interruption| interruption.detected_at >= event.appended_at)
        {
            return None;
        }
        Some(event)
    }

    pub fn from_history(
        sandbox: &Sandbox,
        transitions: &[SandboxEvent],
        controls: &[SandboxControlRecord],
    ) -> Option<Self> {
        let event = Self::event(sandbox, transitions)?;
        let SandboxEventKind::StatusChanged { to, .. } = event.kind else {
            return None;
        };
        let command = match to {
            SandboxStatus::Pausing => Some(SandboxControl::Pause),
            SandboxStatus::Reviving => Some(SandboxControl::Revive),
            _ => None,
        };
        let mut matching = controls.iter().filter(|record| {
            record.envelope.sandbox_id == sandbox.id
                && Some(record.envelope.command) == command
                && matches!(&record.status, SandboxControlStatus::Applied {
                    at, disposition: SandboxControlDisposition::Scheduled, ..
                } if at == &event.appended_at)
        });
        let requested_at = if let Some(record) = matching.next() {
            let SandboxControlStatus::Applied {
                sandbox_revision, ..
            } = record.status
            else {
                return None;
            };
            // A receipt has application time, not event sequence. If another
            // boundary shares that time, it might belong to the prior operation.
            if transitions.iter().any(|previous| {
                previous.seq < event.seq && previous.appended_at >= event.appended_at
            }) || matching.next().is_some()
                || record.validate().is_err()
                || record.envelope.sandbox_created_at != sandbox.created_at
                || sandbox_revision > sandbox.revision
                || match &record.envelope.owner {
                    crate::resource_access::ResourceOwner::Person { subject }
                    | crate::resource_access::ResourceOwner::Organization { subject } => {
                        subject != &sandbox.owner
                    }
                }
            {
                return None;
            }
            record.admitted_at.clone()
        } else {
            event.appended_at.clone()
        };
        // Checkpoint capture allocates authority only while Active. A subsequent
        // lifecycle admission supersedes it; no capture can advance this counter
        // between the admitted transition and failure.
        let generation = sandbox
            .lifecycle
            .next_authority
            .checked_sub(1)
            .filter(|value| *value > 0)?;
        Some(Self {
            operation_id: format!("sandbox-lifecycle-v1:{}:{generation}", sandbox.id),
            requested_at,
        })
    }
}
