/// Immutable for one process incarnation. Existing constructors select Legacy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MaintenancePolicy {
    #[default]
    Legacy,
    ContainFailures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetirementIntent {
    Cooperative,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    Released,
    Superseded { newer_epoch: u64 },
}

/// Scheduling result; a retry never carries cooperative handoff hints.
pub enum MaintenanceStep {
    Confirmed {
        hints: Vec<std::sync::Arc<super::Authority>>,
        next_at: tokio::time::Instant,
    },
    RetryAt(tokio::time::Instant),
    Draining,
}
