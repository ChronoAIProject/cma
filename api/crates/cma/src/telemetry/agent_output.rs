//! Fixed-cardinality observations of one recorder attempt, without payloads.
use layer2_core::sandbox::AgentConversationError;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

tokio::task_local! { static COMMANDS: Arc<Commands>; }

struct Commands {
    count: AtomicU64,
    labels: Mutex<(&'static str, &'static str)>,
}
impl Default for Commands {
    fn default() -> Self {
        Self {
            count: AtomicU64::new(0),
            labels: Mutex::new(("unclassified", "cancelled")),
        }
    }
}
impl Drop for Commands {
    fn drop(&mut self) {
        let (category, outcome) = *self.labels.lock().expect("output metric labels poisoned");
        // A timed-out reconciliation may keep running. Emit once its final
        // inherited observer drops, including those real command attempts.
        metrics::histogram!("cma_agent_output_mongo_commands", "category" => category, "outcome" => outcome)
            .record(self.count.load(Ordering::Relaxed) as f64);
    }
}

pub(crate) fn command_started() {
    // No observer is normal for commands outside an output-recording attempt.
    let _ = COMMANDS.try_with(|commands| commands.count.fetch_add(1, Ordering::Relaxed));
}

#[cfg(test)]
pub(crate) async fn count_commands<F: Future>(future: F) -> (F::Output, u64) {
    let observed = Arc::new(Commands::default());
    let result = COMMANDS.scope(observed.clone(), future).await;
    (result, observed.count.load(Ordering::Relaxed))
}

/// Capture before spawning: task-local state is not inherited by Tokio tasks.
pub(crate) fn inherit_commands<F: Future>(future: F) -> impl Future<Output = F::Output> {
    let inherited = COMMANDS.try_with(Clone::clone).ok();
    async move {
        match inherited {
            Some(commands) => COMMANDS.scope(commands, future).await,
            None => future.await,
        }
    }
}

pub(crate) async fn observe<F, T>(future: F) -> Result<T, AgentConversationError>
where
    F: Future<Output = Result<T, AgentConversationError>>,
{
    let commands = Arc::new(Commands::default());
    let started = Instant::now();
    let (result, category) = COMMANDS
        .scope(
            commands.clone(),
            layer2_core::sandbox::observe_agent_output(future),
        )
        .await;
    let outcome = match &result {
        Ok(_) => "recorded",
        Err(AgentConversationError::Unavailable(_)) => "unavailable",
        Err(AgentConversationError::StaleGeneration) => "stale_generation",
        Err(AgentConversationError::CapacityExceeded) => "capacity",
        Err(_) => "rejected",
    };
    let category = category.label();
    *commands
        .labels
        .lock()
        .expect("output metric labels poisoned") = (category, outcome);
    metrics::counter!("cma_agent_output_record_attempts_total", "category" => category, "outcome" => outcome).increment(1);
    metrics::histogram!("cma_agent_output_record_seconds", "category" => category, "outcome" => outcome).record(started.elapsed().as_secs_f64());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_scope_excludes_unrelated_work_and_counts_errors() {
        let a = Arc::new(Commands::default());
        let b = Arc::new(Commands::default());
        let first = COMMANDS.scope(a.clone(), async {
            command_started();
            tokio::task::yield_now().await;
            command_started();
        });
        let second = COMMANDS.scope(b.clone(), async {
            command_started();
        });
        tokio::join!(first, second);
        command_started();
        assert_eq!(a.count.load(Ordering::Relaxed), 2);
        assert_eq!(b.count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn detached_reconciliation_retains_its_original_command_observer() {
        let observed = Arc::new(Commands::default());
        let (send, receive) = tokio::sync::oneshot::channel();
        let task = COMMANDS.sync_scope(observed.clone(), || {
            command_started();
            tokio::spawn(inherit_commands(async move {
                receive.await.unwrap();
                command_started();
            }))
        });
        assert_eq!(observed.count.load(Ordering::Relaxed), 1);
        send.send(()).unwrap();
        task.await.unwrap();
        assert_eq!(observed.count.load(Ordering::Relaxed), 2);
    }
}
