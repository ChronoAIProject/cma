//! Runtime-only recording observations; no identifiers or checkpoint fields.
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};

tokio::task_local! { static CATEGORY: Arc<AtomicU8>; }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AgentOutputCategory {
    Unclassified,
    AppendedLog,
    DurableAssembly,
    QuietReceipt,
    Replay,
}

impl AgentOutputCategory {
    pub(super) fn observe_fold(has_logs: bool, durable_change: bool) {
        let category = if has_logs {
            Self::AppendedLog
        } else if durable_change {
            Self::DurableAssembly
        } else {
            Self::QuietReceipt
        };
        category.observe();
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unclassified => "unclassified",
            Self::AppendedLog => "appended_log",
            Self::DurableAssembly => "durable_assembly",
            Self::QuietReceipt => "quiet_receipt",
            Self::Replay => "replay",
        }
    }

    pub(super) fn observe(self) {
        // Recording also serves callers without telemetry. Absence of a scope
        // intentionally means there is no observer, not a persistence error.
        let _ = CATEGORY.try_with(|value| value.store(self as u8, Ordering::Relaxed));
    }
}

pub async fn observe_agent_output<F: Future>(future: F) -> (F::Output, AgentOutputCategory) {
    let category = Arc::new(AtomicU8::new(0));
    let result = CATEGORY.scope(category.clone(), future).await;
    let observed = match category.load(Ordering::Relaxed) {
        1 => AgentOutputCategory::AppendedLog,
        2 => AgentOutputCategory::DurableAssembly,
        3 => AgentOutputCategory::QuietReceipt,
        4 => AgentOutputCategory::Replay,
        _ => AgentOutputCategory::Unclassified,
    };
    (result, observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parallel_scopes_keep_categories_separate_and_errors_visible() {
        let a = observe_agent_output(async {
            AgentOutputCategory::QuietReceipt.observe();
            tokio::task::yield_now().await;
            Ok::<_, ()>(())
        });
        let b = observe_agent_output(async {
            AgentOutputCategory::DurableAssembly.observe();
            tokio::task::yield_now().await;
            Err::<(), _>("unavailable")
        });
        let (a, b) = tokio::join!(a, b);
        assert_eq!(a, (Ok(()), AgentOutputCategory::QuietReceipt));
        assert_eq!(
            b,
            (Err("unavailable"), AgentOutputCategory::DurableAssembly)
        );
        assert_eq!(
            observe_agent_output(async {}).await.1,
            AgentOutputCategory::Unclassified
        );
        AgentOutputCategory::Replay.observe();
    }
}
