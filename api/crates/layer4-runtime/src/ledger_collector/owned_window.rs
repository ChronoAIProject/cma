//! Own one admitted collection window independently of the response receiver.
//! Process exit and uncertain outcomes still require A/C's durable held state.
use std::{sync::Arc, time::Duration};
use tokio::sync::oneshot;

use super::{advance, port::CollectorPort, types::*, Admission, Step};

#[derive(Debug, Clone, Copy)]
pub struct WindowBudget {
    pub max_steps: u16,
    pub deadline: Duration,
}

impl Default for WindowBudget {
    fn default() -> Self {
        Self {
            max_steps: 16,
            deadline: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOutcome {
    Stopped(Step),
    /// No release authority. The next invocation resumes original pending work.
    BudgetExhausted,
}

pub struct WindowReply(oneshot::Receiver<Result<WindowOutcome>>);

impl WindowReply {
    pub async fn receive(self) -> Result<WindowOutcome> {
        self.0.await.map_err(|_| Refusal::UnknownOutcome)?
    }
}

/// Dropping the reply cannot cancel the admitted job. Do not expose an abortable
/// JoinHandle as effect authority. Every backend call must retain its own owned
/// guards in detached storage work, including when this job reaches its deadline.
pub fn start<P: CollectorPort + 'static>(
    admission: Admission,
    port: Arc<P>,
    budget: WindowBudget,
) -> WindowReply {
    let (send, receive) = oneshot::channel();
    if budget.max_steps == 0
        || budget.max_steps > 64
        || budget.deadline.is_zero()
        || budget.deadline > Duration::from_secs(30)
    {
        let _ = send.send(Err(Refusal::InvalidEvidence));
        return WindowReply(receive);
    }
    // Runtime shutdown can terminate this task; it never clears durable holds.
    let job = tokio::spawn(async move {
        let result =
            tokio::time::timeout(budget.deadline, drive(admission, &*port, budget.max_steps))
                .await
                .unwrap_or(Err(Refusal::UnknownOutcome));
        let _ = send.send(result);
    });
    drop(job); // Tokio detaches; response cancellation is not job cancellation.
    WindowReply(receive)
}

async fn drive<P: CollectorPort>(
    admission: Admission,
    port: &P,
    max_steps: u16,
) -> Result<WindowOutcome> {
    let mut checkpoint = port.load().await?;
    if !checkpoint.valid() {
        return Err(Refusal::InvalidEvidence);
    }
    if checkpoint.pending.is_none() && !admission.new_work {
        return Ok(WindowOutcome::Stopped(Step::Disabled));
    }
    let fence = checkpoint.fence;
    let hold = port.acquire(&checkpoint).await?;
    for _ in 0..max_steps {
        // A renewed claim is a new coordinator, never authority inherited by
        // an old job merely because its next load sees a newer fence.
        if checkpoint.fence != fence {
            return Err(Refusal::Conflict);
        }
        let result = advance(port, &checkpoint, hold.clone()).await?;
        if matches!(result, Step::GroupSettled | Step::InventoryComplete) {
            return Ok(WindowOutcome::Stopped(result));
        }
        checkpoint = port.load().await?;
    }
    Ok(WindowOutcome::BudgetExhausted)
}
