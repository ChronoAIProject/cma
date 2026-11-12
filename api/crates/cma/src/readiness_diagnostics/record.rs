use layer2_core::diagnostic_context::DiagnosticContext;
use std::sync::Arc;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    pub mono_us: u64,
    pub unix_us: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Metadata {
    pub context: DiagnosticContext,
    pub pool: &'static str,
    pub command: &'static str,
    pub collection: &'static str,
    pub hold_hash: Arc<str>,
    pub operation: u64,
    pub check: u64,
    pub session: bool,
    pub transaction: bool,
}

impl Metadata {
    pub(crate) fn new(pool: &'static str, mut context: DiagnosticContext) -> Self {
        // Context is supplied by registered internal call sites, never raw paths.
        // Still bound retained string lengths independently of caller behavior.
        if context.route.len() > 160 {
            context.route = "overflow".into();
        }
        if context.request_hash.len() != 64
            || !context.request_hash.bytes().all(|c| c.is_ascii_hexdigit())
        {
            context.request_hash = "none".into();
        }
        Self {
            context,
            pool,
            command: "none",
            collection: "none",
            hold_hash: "none".into(),
            operation: 0,
            check: 0,
            session: false,
            transaction: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub stamp: Stamp,
    pub metadata: Metadata,
    pub event: &'static str,
    pub duration_us: u64,
}

#[derive(Debug)]
pub(crate) enum Record {
    Attempt {
        stamp: Stamp,
        check: u64,
        event: &'static str,
        outcome: &'static str,
        phase: &'static str,
        elapsed_us: u64,
        budget_us: u64,
        deadline_us: u64,
        wake_lag_us: u64,
        overshoot_us: u64,
        phase_elapsed_us: u64,
        operation: u64,
    },
    Response {
        stamp: Stamp,
        check: u64,
        reason: &'static str,
        age_us: u64,
        age_known: bool,
        check_duration_us: u64,
    },
    StateChanged {
        stamp: Stamp,
        reason: &'static str,
        duration_ms: u64,
    },
    Timer {
        stamp: Stamp,
        lag_us: u64,
    },
}

impl Record {
    pub(super) fn write(self, process: &str, queue_delay_us: u64) {
        match self {
            Self::Attempt {
                stamp,
                check,
                event,
                outcome,
                phase,
                elapsed_us,
                budget_us,
                deadline_us,
                wake_lag_us,
                overshoot_us,
                phase_elapsed_us,
                operation,
            } => {
                tracing::info!(target: "readiness_diagnostic", parent: None,
                    process_hash=process, observed_unix_us=stamp.unix_us, process_monotonic_us=stamp.mono_us,
                    event_sequence=stamp.sequence, check_sequence=check, operation_sequence=operation,
                    event, outcome, cancelled=matches!(outcome, "deadline" | "cancelled"), phase, elapsed_us, budget_us, deadline_us, wake_lag_us, deadline_overshoot_us=overshoot_us,
                    phase_elapsed_us, queue_delay_us, "Readiness attempt evidence");
            }
            Self::Response {
                stamp,
                check,
                reason,
                age_us,
                age_known,
                check_duration_us,
            } => {
                tracing::info!(target: "readiness_diagnostic", parent: None,
                    process_hash=process, observed_unix_us=stamp.unix_us, process_monotonic_us=stamp.mono_us,
                    event_sequence=stamp.sequence, check_sequence=check, reason,
                    cache_age_us=age_us, age_known, check_duration_us, queue_delay_us,
                    route="GET /ready", "Readiness response evidence");
            }
            Self::StateChanged {
                stamp,
                reason,
                duration_ms,
            } => {
                tracing::info!(target: "cma::health", parent: None, observed_unix_us=stamp.unix_us, queue_delay_us,
                    serving_capacity=reason == "ready", duration_ms, reason, "readiness dependency state changed");
            }
            Self::Timer { stamp, lag_us } => {
                tracing::info!(target: "readiness_diagnostic", parent: None,
                    process_hash=process, observed_unix_us=stamp.unix_us, process_monotonic_us=stamp.mono_us,
                    event_sequence=stamp.sequence, lag_us, queue_delay_us, "Readiness timer lag");
            }
        }
    }
}

impl Event {
    pub(super) fn write(&self, process: &str, snapshot: u64, queue_delay_us: u64) {
        let m = &self.metadata;
        tracing::info!(target: "readiness_diagnostic_ring", parent: None,
            process_hash=process, snapshot_check_sequence=snapshot, observed_unix_us=self.stamp.unix_us,
            process_monotonic_us=self.stamp.mono_us, event_sequence=self.stamp.sequence,
            check_sequence=m.check, operation_sequence=m.operation, hold_hash=%m.hold_hash,
            route=m.context.route.as_ref(), request_hash=%m.context.request_hash,
            worker=m.context.worker, phase=m.context.phase, pool=m.pool,
            command=m.command, collection=m.collection, session=m.session, transaction=m.transaction, event=self.event,
            duration_us=self.duration_us, queue_delay_us, "Readiness Mongo metadata");
    }
}
