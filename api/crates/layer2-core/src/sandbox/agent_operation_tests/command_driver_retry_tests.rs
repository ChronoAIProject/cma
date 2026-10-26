use super::*;

struct UnavailableOnce {
    inner: Arc<Execution>,
    checks: AtomicUsize,
    entered: Notify,
}

#[async_trait::async_trait]
impl AgentCommandExecution for UnavailableOnce {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if self.checks.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            CommandReadiness::Retry
        } else {
            self.inner.check_queued(command).await
        }
    }

    async fn prepare_queued(&self, command: &AdmittedAgentCommand) {
        self.inner.prepare_queued(command).await;
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.inner.execute(command).await
    }
}

#[tokio::test(start_paused = true)]
async fn scheduled_retries_release_the_slot_without_caching_authority_or_claims() {
    for status in [AgentStatus::Ready, AgentStatus::Paused] {
        let (store, agent, parent) = fixture(status).await;
        let admitted = admit(
            &store,
            &agent,
            &parent,
            "scheduled-retry",
            AgentCommand::message("preserve this queued message"),
        )
        .await;
        let (sender, inputs) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let inner = Execution::new(store.clone(), operations.clone(), Behavior::Execute);
        let execution: Arc<dyn AgentCommandExecution> = if status == AgentStatus::Ready {
            Arc::new(UnavailableOnce {
                inner,
                checks: AtomicUsize::new(0),
                entered: Notify::new(),
            })
        } else {
            inner
        };
        let worker = AgentCommandWorker::new(store.clone(), execution, "scheduled".into()).unwrap();
        let id = &admitted.receipt.envelope.command_id;
        let started = tokio::time::Instant::now();
        let cooldown = tokio::time::timeout(Duration::from_millis(1), worker.run_scheduled(id))
            .await
            .expect("retry pacing must not occupy the worker slot")
            .unwrap();
        assert_eq!(cooldown, Duration::from_secs(15));
        assert_eq!(tokio::time::Instant::now(), started);
        assert_eq!(store.get_command(id).await.unwrap(), Some(admitted.clone()));
        assert!(inputs.lock().unwrap().is_empty());

        // A later attempt obtains fresh authority even when no cooldown elapsed
        // (for example, because the process restarted).
        let revoked = Execution::new(store.clone(), operations, Behavior::Refuse);
        assert_eq!(
            revoked.worker("restart").run_scheduled(id).await.unwrap(),
            Duration::ZERO
        );
        let current = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(current.receipt, admitted.receipt);
        assert_eq!(current.queue_deadline, admitted.queue_deadline);
        assert_eq!(
            current.work.failure.unwrap().code,
            CommandFailureCode::AuthorityRevoked
        );
        assert!(current.work.lease.is_none());
        assert!(current.work.dispatch.is_none());
        assert!(inputs.lock().unwrap().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn transient_authority_preserves_receipt_and_retries_after_bounded_pacing() {
    let (store, agent, parent) = fixture(AgentStatus::Ready).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "retry",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let inner = Execution::new(store.clone(), operations, Behavior::Execute);
    let execution = Arc::new(UnavailableOnce {
        inner,
        checks: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let worker =
        AgentCommandWorker::new(store.clone(), execution.clone(), "original".into()).unwrap();
    let id = admitted.receipt.envelope.command_id.clone();
    let run_id = id.clone();
    let task = tokio::spawn(async move { worker.run_one(&run_id).await });
    execution.entered.notified().await;
    tokio::task::yield_now().await;
    assert_eq!(
        store.get_command(&id).await.unwrap(),
        Some(admitted.clone())
    );
    assert!(inputs.lock().unwrap().is_empty());
    tokio::time::advance(Duration::from_secs(14)).await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    task.await.unwrap().unwrap();
    assert_eq!(execution.checks.load(Ordering::SeqCst), 1);
    let a = AgentCommandWorker::new(store.clone(), execution.clone(), "restart".into()).unwrap();
    let b = AgentCommandWorker::new(store.clone(), execution.clone(), "replica".into()).unwrap();
    let (a, b) = tokio::join!(a.run_one(&id), b.run_one(&id));
    a.unwrap();
    b.unwrap();
    let current = store.get_command(&id).await.unwrap().unwrap();
    assert_eq!(current.receipt, admitted.receipt);
    assert_eq!(current.queue_deadline, admitted.queue_deadline);
    assert!(current.work.failure.is_none());
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert!(execution.checks.load(Ordering::SeqCst) > 1);
}

#[tokio::test(start_paused = true)]
async fn cancellation_of_waiting_preparation_keeps_work_queued_and_rechecks_revocation() {
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let admitted = admit(
        &store,
        &agent,
        &parent,
        "paused",
        AgentCommand::message("one"),
    )
    .await;
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations.clone(), Behavior::Execute);
    let worker = execution.worker("waiting");
    let id = admitted.receipt.envelope.command_id.clone();
    let run_id = id.clone();
    let task = tokio::spawn(async move { worker.run_one(&run_id).await });
    while execution.preparations.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(14)).await;
    assert!(!task.is_finished());
    assert_eq!(
        store.get_command(&id).await.unwrap(),
        Some(admitted.clone())
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let revoked = Execution::new(store.clone(), operations, Behavior::Refuse);
    revoked.worker("restart").run_one(&id).await.unwrap();
    let current = store.get_command(&id).await.unwrap().unwrap();
    assert_eq!(current.receipt, admitted.receipt);
    assert_eq!(current.queue_deadline, admitted.queue_deadline);
    assert_eq!(
        current.work.failure.unwrap().code,
        CommandFailureCode::AuthorityRevoked
    );
    assert!(current.work.dispatch.is_none());
    assert!(current.work.lease.is_none());
    assert!(inputs.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn unclaimed_pacing_is_capped_by_the_original_queue_deadline() {
    use super::super::ledger_tests::{record, reservation};
    let (store, agent, parent) = fixture(AgentStatus::Paused).await;
    let mut submission = reservation(&agent, &parent, "short-deadline");
    submission.admitted_at = crate::now_rfc3339();
    submission.queue_deadline =
        crate::rfc3339_at(std::time::SystemTime::now() + Duration::from_secs(3));
    let admitted = record(store.reserve_command(submission).await.unwrap());
    let (sender, inputs) = recording_sender(Ok(()));
    let operations = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let execution = Execution::new(store.clone(), operations, Behavior::Execute);
    let worker = execution.worker("deadline");
    let id = admitted.receipt.envelope.command_id.clone();
    let run_id = id.clone();
    let task = tokio::spawn(async move { worker.run_one(&run_id).await });
    while execution.preparations.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::task::yield_now().await;
    assert!(task.is_finished());
    task.await.unwrap().unwrap();
    assert_eq!(store.get_command(&id).await.unwrap(), Some(admitted));
    assert!(inputs.lock().unwrap().is_empty());
}
