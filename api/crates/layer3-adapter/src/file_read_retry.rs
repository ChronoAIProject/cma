//! One receiver retry and one negotiation deadline for an owned file read.
use std::{cell::Cell, future::Future, time::Duration};
use tokio::time::Instant;

const NEGOTIATION_WINDOW: Duration = Duration::from_secs(8);

struct Budget {
    deadline: Instant,
    used: Cell<bool>,
}

tokio::task_local! { static BUDGET: Budget; }

pub async fn scope<T>(operation: impl Future<Output = T>) -> T {
    BUDGET
        .scope(
            Budget {
                deadline: Instant::now() + NEGOTIATION_WINDOW,
                used: Cell::new(false),
            },
            operation,
        )
        .await
}

pub(crate) fn deadline() -> Option<Instant> {
    BUDGET.try_with(|budget| budget.deadline).ok()
}

pub(crate) fn take() -> bool {
    BUDGET
        .try_with(|budget| Instant::now() < budget.deadline && !budget.used.replace(true))
        .unwrap_or(false)
}
