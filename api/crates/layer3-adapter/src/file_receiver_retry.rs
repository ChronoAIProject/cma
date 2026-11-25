//! One pre-dispatch receiver retry for an owned file read or conditional edit.
use std::{cell::Cell, future::Future, time::Duration};
use tokio::time::Instant;

use crate::wire::RequestFrame;

const NEGOTIATION_WINDOW: Duration = Duration::from_secs(8);

enum Operation {
    Read,
    ConditionalWrite,
}

struct Budget {
    deadline: Instant,
    used: Cell<bool>,
    operation: Operation,
}

tokio::task_local! { static BUDGET: Budget; }

pub async fn scope_read<T>(execute: impl Future<Output = T>) -> T {
    scope(Operation::Read, execute).await
}

pub async fn scope_conditional_write<T>(execute: impl Future<Output = T>) -> T {
    scope(Operation::ConditionalWrite, execute).await
}

async fn scope<T>(operation: Operation, execute: impl Future<Output = T>) -> T {
    BUDGET
        .scope(
            Budget {
                deadline: Instant::now() + NEGOTIATION_WINDOW,
                used: Cell::new(false),
                operation,
            },
            execute,
        )
        .await
}

pub(crate) fn deadline(request: &RequestFrame) -> Option<Instant> {
    BUDGET
        .try_with(|budget| {
            let eligible = match (&budget.operation, request) {
                (_, RequestFrame::RuntimeStatus(_))
                | (
                    Operation::Read,
                    RequestFrame::ListAgentFiles(_) | RequestFrame::ReadAgentFile(_),
                ) => true,
                (Operation::ConditionalWrite, RequestFrame::WriteAgentFile(edit)) => {
                    !edit.directory
                        && edit.expected_sha256.as_deref().is_some_and(|digest| {
                            digest.len() == 64
                                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                }
                _ => false,
            };
            eligible.then_some(budget.deadline)
        })
        .ok()
        .flatten()
}

pub(crate) fn take() -> bool {
    BUDGET
        .try_with(|budget| Instant::now() < budget.deadline && !budget.used.replace(true))
        .unwrap_or(false)
}
