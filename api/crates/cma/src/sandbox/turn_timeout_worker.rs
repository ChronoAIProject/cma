//! Retained deadlines run independently of admission switches.
use std::{sync::Arc, time::Duration};

use layer2_core::{
    agent_ownership::OwnershipCoordinator,
    sandbox::{
        agent_io::AgentCommandStore,
        store::{AgentConversationStore, SandboxStoreError},
        turn_timeout::TurnTimeoutSettlement,
    },
};
use tokio::{sync::watch, task::JoinHandle, time::MissedTickBehavior};

use crate::{
    background::{self, BackgroundWork},
    store::{turn_timeouts::discovery, MongoSandboxStore},
    worker_supervision::{self, Activity},
};

const NAME: &str = "agent_turn_timeout";
const INTERVAL: Duration = Duration::from_secs(2);
const SWEEP_BUDGET: Duration = Duration::from_secs(5);

pub(super) struct Worker {
    store: Arc<MongoSandboxStore>,
    ownership: Option<Arc<OwnershipCoordinator>>,
    after: Option<discovery::Candidate>,
    work: BackgroundWork,
}

impl Worker {
    pub(super) fn new(
        store: Arc<MongoSandboxStore>,
        ownership: Option<Arc<OwnershipCoordinator>>,
    ) -> Self {
        Self {
            store,
            ownership,
            after: None,
            work: BackgroundWork::new(background::shared(), NAME, SWEEP_BUDGET, INTERVAL),
        }
    }

    pub(super) fn spawn(
        self,
        shutdown: watch::Receiver<bool>,
        activity: Activity,
    ) -> JoinHandle<()> {
        worker_supervision::spawn(
            self,
            activity,
            shutdown,
            |worker, activity, mut shutdown| {
                Box::pin(async move {
                    let mut ticker = tokio::time::interval(INTERVAL);
                    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                    loop {
                        activity.waiting(INTERVAL + SWEEP_BUDGET, "turn_policy_scan");
                        tokio::select! {
                            _ = shutdown.changed() => break,
                            _ = ticker.tick() => {
                                activity.scheduled();
                                let _scan = activity.reconciliation();
                                let work = worker.work.clone();
                                if work.run_resumable(|| worker.sweep()).await.is_err() {
                                    outcome("scan_unavailable");
                                }
                            }
                        }
                    }
                })
            },
            |_| Box::pin(async {}),
        )
    }

    async fn sweep(&mut self) -> Result<bool, SandboxStoreError> {
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        let rows = discovery::page(&self.store, self.after.as_ref(), now).await?;
        let complete = rows.len() < discovery::PAGE_SIZE as usize;
        for row in rows {
            // Advance before retryable IO; an unavailable early row cannot
            // starve later obligations. The authoritative row is never removed.
            self.after = Some(row.clone());
            let operation = async {
                let Some(ownership) = &self.ownership else {
                    return Err(SandboxStoreError::Unavailable(
                        "turn policy owner unavailable".into(),
                    ));
                };
                let Some(authority) = ownership.local(&row.id).await else {
                    outcome("owner_elsewhere");
                    return Ok(());
                };
                authority
                    .run(settle_candidate(&self.store, &row))
                    .await
                    .map_err(|_| SandboxStoreError::Conflict)?
            };
            match tokio::time::timeout(Duration::from_secs(2), operation).await {
                Ok(Ok(())) => {}
                Ok(Err(SandboxStoreError::Conflict)) => outcome("changed"),
                Ok(Err(_)) => outcome("terminal_unconfirmed"),
                Err(_) => outcome("attempt_deadline"),
            }
            metrics::histogram!("cma_turn_policy_overdue_seconds")
                .record(now.saturating_sub(row.deadline_at_ms).max(0) as f64 / 1_000.0);
        }
        if complete {
            self.after = None;
        }
        Ok(complete)
    }
}

pub(crate) async fn settle_candidate(
    store: &MongoSandboxStore,
    candidate: &discovery::Candidate,
) -> Result<(), SandboxStoreError> {
    let Some(snapshot) = store.agent_message_snapshot(&candidate.id).await? else {
        return Ok(());
    };
    let Some(policy) = &snapshot.agent.conversation.turn_timeout else {
        return Err(SandboxStoreError::Conflict);
    };
    if !policy.is_armed() || policy.deadline_at_ms() != candidate.deadline_at_ms {
        return Err(SandboxStoreError::Conflict);
    }
    let command = store
        .get_command(&policy.binding().command_id)
        .await
        .map_err(|_| SandboxStoreError::Unavailable("turn policy command unavailable".into()))?
        .ok_or(SandboxStoreError::Conflict)?;
    let revision = policy.revision();
    let proof =
        TurnTimeoutSettlement::prepare(snapshot, command, revision, layer2_core::now_rfc3339())
            .map_err(|_| SandboxStoreError::Conflict)?;
    store.commit_turn_timeout(&proof).await?;
    outcome("terminal_recorded");
    Ok(())
}

fn outcome(value: &'static str) {
    metrics::counter!("cma_turn_policy_reconciliation_total", "outcome" => value).increment(1);
}
