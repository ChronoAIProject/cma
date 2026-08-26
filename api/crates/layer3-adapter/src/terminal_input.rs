//! Receipt-bearing delivery toward a runtime terminal.

use std::future::Future;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::task_retirement::TaskRetirement;
use crate::{AdapterError, TerminalInput};

/// A local runtime-link writer must settle one item well inside the durable
/// interaction lease held by the core.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(2);

struct PendingTerminalInput {
    input: TerminalInput,
    delivered: oneshot::Sender<Result<(), AdapterError>>,
}

struct AbortWriterOnDrop {
    abort: tokio::task::AbortHandle,
    armed: bool,
}

impl AbortWriterOnDrop {
    fn new(abort: tokio::task::AbortHandle) -> Self {
        Self { abort, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AbortWriterOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.abort.abort();
        }
    }
}

/// The sending half of a runtime terminal link.
///
/// Enqueueing into a Tokio channel does not mean a detached writer reached its
/// socket. This sender waits for that writer's typed result, so the core can
/// retain its durable interaction fence through the actual link write.
#[derive(Clone)]
pub struct TerminalInputSender {
    inner: mpsc::Sender<PendingTerminalInput>,
    abort: tokio::task::AbortHandle,
    retirement: TaskRetirement,
}

impl std::fmt::Debug for TerminalInputSender {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalInputSender")
            .field("closed", &self.inner.is_closed())
            .field("capacity", &self.inner.capacity())
            .finish()
    }
}

impl TerminalInputSender {
    /// Run one ordered terminal writer.
    ///
    /// The handler returns only after writing the item to the runtime link. A
    /// failed write closes this generation; later callers then fail at enqueue
    /// instead of feeding a task whose socket is already known to be dead.
    pub fn spawn<F, Fut>(capacity: usize, mut handler: F) -> Self
    where
        F: FnMut(TerminalInput) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), AdapterError>> + Send + 'static,
    {
        let (inner, mut pending) = mpsc::channel::<PendingTerminalInput>(capacity);
        let retirement = TaskRetirement::default();
        let retired = retirement.guard();
        let task = tokio::spawn(async move {
            let _retired = retired;
            while let Some(PendingTerminalInput { input, delivered }) = pending.recv().await {
                let result = handler(input).await;
                let failed = result.is_err();
                if failed {
                    pending.close();
                }
                let _ = delivered.send(result);
                if failed {
                    break;
                }
            }
        });
        let abort = task.abort_handle();
        Self {
            inner,
            abort,
            retirement,
        }
    }

    /// Send one item and wait until the runtime-link writer settles it.
    pub async fn send(&self, input: TerminalInput) -> Result<(), AdapterError> {
        self.send_with_deadline(input, DELIVERY_DEADLINE).await
    }

    async fn send_with_deadline(
        &self,
        input: TerminalInput,
        deadline: Duration,
    ) -> Result<(), AdapterError> {
        let (delivered, receipt) = oneshot::channel();
        self.inner
            .try_send(PendingTerminalInput { input, delivered })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AdapterError::Unavailable(
                    "the terminal input queue is not being consumed".to_string(),
                ),
                mpsc::error::TrySendError::Closed(_) => {
                    AdapterError::Unavailable("the terminal input task has ended".to_string())
                }
            })?;

        // Once an item is admitted, losing its waiter must poison this exact
        // writer generation. Otherwise a canceled request could release its
        // outer durable fence while this task later writes the item.
        let mut abort_on_drop = AbortWriterOnDrop::new(self.abort.clone());
        let result = match tokio::time::timeout(deadline, receipt).await {
            Err(_) => {
                self.retire().await;
                Err(AdapterError::Unavailable(
                    "the terminal input task did not deliver the operation".to_string(),
                ))
            }
            Ok(Err(_)) => {
                self.retire().await;
                Err(AdapterError::Unavailable(
                    "the terminal input task ended before delivery".to_string(),
                ))
            }
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => {
                self.retire().await;
                Err(error)
            }
        };
        abort_on_drop.disarm();
        result
    }

    pub(crate) fn same_channel(&self, other: &Self) -> bool {
        self.inner.same_channel(&other.inner)
    }

    pub(crate) async fn retire(&self) {
        self.abort.abort();
        self.retirement.wait().await;
    }

    pub(crate) async fn wait_retired(&self) {
        self.retirement.wait().await;
    }

    pub(crate) fn downgrade(&self) -> WeakTerminalInputSender {
        WeakTerminalInputSender {
            inner: self.inner.downgrade(),
            abort: self.abort.clone(),
            retirement: self.retirement.clone(),
        }
    }
}

pub(crate) struct WeakTerminalInputSender {
    inner: mpsc::WeakSender<PendingTerminalInput>,
    abort: tokio::task::AbortHandle,
    retirement: TaskRetirement,
}

impl WeakTerminalInputSender {
    pub(crate) fn upgrade(&self) -> Option<TerminalInputSender> {
        self.inner.upgrade().map(|inner| TerminalInputSender {
            inner,
            abort: self.abort.clone(),
            retirement: self.retirement.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use tokio::sync::{oneshot, Notify};

    use super::*;

    #[tokio::test]
    async fn writer_failure_is_returned_and_closes_the_generation() {
        let sender = TerminalInputSender::spawn(1, |_input| async {
            Err(AdapterError::Unavailable("socket write failed".into()))
        });

        assert!(matches!(
            sender.send(TerminalInput::Bytes(vec![1])).await,
            Err(AdapterError::Unavailable(detail)) if detail == "socket write failed"
        ));
        tokio::task::yield_now().await;
        assert!(matches!(
            sender.send(TerminalInput::Bytes(vec![2])).await,
            Err(AdapterError::Unavailable(detail)) if detail == "the terminal input task has ended"
        ));
    }

    #[tokio::test]
    async fn canceling_a_receipt_wait_poisons_its_writer_generation() {
        let (started, started_rx) = oneshot::channel();
        let mut started = Some(started);
        let resume = Arc::new(Notify::new());
        let wrote = Arc::new(AtomicBool::new(false));
        let sender = TerminalInputSender::spawn(1, {
            let resume = Arc::clone(&resume);
            let wrote = Arc::clone(&wrote);
            move |_input| {
                let started = started.take();
                let resume = Arc::clone(&resume);
                let wrote = Arc::clone(&wrote);
                async move {
                    if let Some(started) = started {
                        let _ = started.send(());
                    }
                    resume.notified().await;
                    wrote.store(true, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        let delivery = tokio::spawn({
            let sender = sender.clone();
            async move { sender.send(TerminalInput::Bytes(vec![1])).await }
        });
        started_rx.await.expect("the writer started the operation");
        delivery.abort();
        assert!(delivery
            .await
            .expect_err("delivery was canceled")
            .is_cancelled());
        sender.wait_retired().await;

        resume.notify_waiters();
        tokio::task::yield_now().await;
        assert!(!wrote.load(Ordering::SeqCst));
        assert!(matches!(
            sender.send(TerminalInput::Bytes(vec![2])).await,
            Err(AdapterError::Unavailable(detail)) if detail == "the terminal input task has ended"
        ));
    }

    #[tokio::test]
    async fn a_timeout_waits_for_writer_retirement_before_returning() {
        let resume = Arc::new(Notify::new());
        let wrote = Arc::new(AtomicBool::new(false));
        let sender = TerminalInputSender::spawn(1, {
            let resume = Arc::clone(&resume);
            let wrote = Arc::clone(&wrote);
            move |_input| {
                let resume = Arc::clone(&resume);
                let wrote = Arc::clone(&wrote);
                async move {
                    resume.notified().await;
                    wrote.store(true, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        assert!(matches!(
            sender
                .send_with_deadline(TerminalInput::Bytes(vec![1]), Duration::from_millis(10))
                .await,
            Err(AdapterError::Unavailable(detail))
                if detail == "the terminal input task did not deliver the operation"
        ));
        assert!(sender.inner.is_closed());
        sender.wait_retired().await;

        resume.notify_waiters();
        tokio::task::yield_now().await;
        assert!(!wrote.load(Ordering::SeqCst));
    }
}
