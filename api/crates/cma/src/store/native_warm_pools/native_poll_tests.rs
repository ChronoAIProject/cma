use super::*;
use crate::release_authority::{native_poll_testing, Result as AuthorityResult};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex as StdMutex,
};
use tokio::sync::Notify;

enum Execution {
    Fresh,
    Uncertain,
    AwaitCancellation,
}

struct Transactions {
    command: NativePollCommand,
    receipt: NativePollReceipt,
    outcome: Execution,
    durable: StdMutex<Option<NativePollReceipt>>,
    prepare_calls: AtomicUsize,
    execute_calls: AtomicUsize,
    reconcile_calls: AtomicUsize,
    entered: Notify,
}

impl Transactions {
    fn new(target: Option<PoolTarget>, outcome: Execution) -> Self {
        let (command, receipt) = native_poll_testing::fixture(NativePollLane::Background, target);
        Self {
            command,
            receipt,
            outcome,
            durable: StdMutex::new(None),
            prepare_calls: AtomicUsize::new(0),
            execute_calls: AtomicUsize::new(0),
            reconcile_calls: AtomicUsize::new(0),
            entered: Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl PollTransactions for Transactions {
    async fn prepare(
        &self,
        lane: NativePollLane,
        previous: Option<&NativePollReceipt>,
    ) -> AuthorityResult<NativePollCommand> {
        assert_eq!(lane, NativePollLane::Background);
        assert!(
            previous.is_none(),
            "these tests never admit a successor command"
        );
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.command.clone())
    }

    async fn execute(&self, command: &NativePollCommand) -> AuthorityResult<NativePollExecution> {
        assert_eq!(command.key(), self.command.key());
        self.execute_calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        match self.outcome {
            Execution::Fresh => Ok(NativePollExecution::Fresh(self.receipt.clone())),
            Execution::Uncertain => Ok(NativePollExecution::Uncertain),
            Execution::AwaitCancellation => std::future::pending().await,
        }
    }

    async fn reconcile(
        &self,
        command: &NativePollCommand,
    ) -> AuthorityResult<Option<NativePollReceipt>> {
        assert_eq!(command.key(), self.command.key());
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.durable.lock().unwrap().clone())
    }
}

fn target() -> PoolTarget {
    let mut target = crate::warm_pools::tests::target();
    target.id = format!("wp_{}", "a".repeat(64));
    target.document_schema_version = 3;
    target
}

#[tokio::test]
async fn cancelled_poll_reconciles_the_saved_command_without_positive_redispatch() {
    let state = Arc::new(Mutex::new(PendingPoll::default()));
    let authority = Arc::new(Transactions::new(
        Some(target()),
        Execution::AwaitCancellation,
    ));
    let task_state = state.clone();
    let task_authority = authority.clone();
    let task = tokio::spawn(async move {
        poll(
            &task_state,
            task_authority.as_ref(),
            NativePollLane::Background,
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        authority.entered.notified(),
    )
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(state.lock().await.command.is_some());

    assert!(poll(&state, authority.as_ref(), NativePollLane::Background)
        .await
        .is_err());
    *authority.durable.lock().unwrap() = Some(authority.receipt.clone());
    assert!(poll(&state, authority.as_ref(), NativePollLane::Background)
        .await
        .is_err());
    assert_eq!(authority.prepare_calls.load(Ordering::SeqCst), 1);
    assert_eq!(authority.execute_calls.load(Ordering::SeqCst), 1);
    assert_eq!(authority.reconcile_calls.load(Ordering::SeqCst), 2);
    assert!(state.lock().await.command.is_some());
    assert!(state.lock().await.delivered.is_none());
}

#[tokio::test]
async fn uncertain_empty_poll_waits_for_its_historical_result_without_rescanning() {
    let state = Mutex::new(PendingPoll::default());
    let authority = Transactions::new(None, Execution::Uncertain);
    assert!(poll(&state, &authority, NativePollLane::Background)
        .await
        .is_err());
    for _ in 0..3 {
        assert!(poll(&state, &authority, NativePollLane::Background)
            .await
            .is_err());
    }
    let (_, foreign) = native_poll_testing::fixture(NativePollLane::StartupAgent, None);
    *authority.durable.lock().unwrap() = Some(foreign);
    assert!(poll(&state, &authority, NativePollLane::Background)
        .await
        .is_err());
    assert!(state.lock().await.command.is_some());
    *authority.durable.lock().unwrap() = Some(authority.receipt.clone());
    assert!(poll(&state, &authority, NativePollLane::Background)
        .await
        .unwrap()
        .is_none());
    assert_eq!(authority.prepare_calls.load(Ordering::SeqCst), 1);
    assert_eq!(authority.execute_calls.load(Ordering::SeqCst), 1);
    assert_eq!(authority.reconcile_calls.load(Ordering::SeqCst), 5);
    let saved = state.lock().await;
    assert!(saved.command.is_none());
    assert_eq!(
        saved.previous.as_ref().unwrap().key(),
        authority.receipt.key()
    );
}

#[tokio::test]
async fn fresh_positive_lease_is_delivered_once_and_only_exact_finish_clears_it() {
    let state = Mutex::new(PendingPoll::default());
    let authority = Transactions::new(Some(target()), Execution::Fresh);
    let delivered = poll(&state, &authority, NativePollLane::Background)
        .await
        .unwrap()
        .unwrap();
    assert!(poll(&state, &authority, NativePollLane::Background)
        .await
        .is_err());
    assert_eq!(authority.execute_calls.load(Ordering::SeqCst), 1);
    for mismatch in 0..3 {
        let mut changed = delivered.clone();
        match mismatch {
            0 => changed.id.push_str("-replacement"),
            1 => changed.revision += 1,
            _ => changed.lease = Some("different-lease".into()),
        }
        state.lock().await.acknowledged_finish(&changed);
        assert!(poll(&state, &authority, NativePollLane::Background)
            .await
            .is_err());
    }
    assert_eq!(authority.prepare_calls.load(Ordering::SeqCst), 1);
    state.lock().await.acknowledged_finish(&delivered);
    assert!(state.lock().await.delivered.is_none());
    assert_eq!(
        state.lock().await.previous.as_ref().unwrap().key(),
        authority.receipt.key()
    );
}
