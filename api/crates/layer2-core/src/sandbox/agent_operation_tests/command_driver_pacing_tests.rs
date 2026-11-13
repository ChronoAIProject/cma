use super::*;

#[tokio::test(start_paused = true)]
async fn worker_dispatched_cooldown_and_head_fallback_preserve_full_reconciliation() {
    for delay in [Duration::ZERO, Duration::from_secs(1)] {
        let (store, agent, parent) = fixture(AgentStatus::Ready).await;
        let admitted = admit(
            &store,
            &agent,
            &parent,
            "paced",
            AgentCommand::message("one"),
        )
        .await;
        let (sender, _) = recording_sender(Ok(()));
        let operations = AgentOperationService::new(
            store.clone(),
            TestRuntimeRegistry::new(full_capabilities(), sender),
        );
        let execution = Execution::new(store.clone(), operations, Behavior::Execute);
        let id = &admitted.receipt.envelope.command_id;
        execution
            .worker("dispatch")
            .run_scheduled(id)
            .await
            .unwrap();
        let uncertain = Arc::new(UncertainStore {
            inner: store.clone(),
            fail_reads: AtomicBool::new(false),
            head_reads: AtomicUsize::new(0),
            head_timeout: AtomicBool::new(false),
            lose_renewal_reply: false,
            releases: AtomicUsize::new(0),
        });
        let worker = AgentCommandWorker::with_recheck_delay(
            uncertain.clone(),
            execution,
            "paced".into(),
            delay,
        )
        .unwrap();
        assert_eq!(worker.run_scheduled(id).await.unwrap(), delay);
        let page = PageRequest::new(50, None).unwrap();
        let discovery = worker.discover_scheduled(&page).await.unwrap();
        assert!(!discovery.heads_failed);
        assert_eq!(discovery.page.items.len(), 1);
        assert_eq!(discovery.page.items[0].reconcile_current, !delay.is_zero());
        assert!(discovery.page.items[0].dispatched);
        assert_eq!(
            uncertain.head_reads.load(Ordering::SeqCst),
            usize::from(!delay.is_zero())
        );
        for timeout in [false, true] {
            uncertain.fail_reads.store(!timeout, Ordering::SeqCst);
            uncertain.head_timeout.store(timeout, Ordering::SeqCst);
            let discovery = worker.discover_scheduled(&page).await.unwrap();
            assert_eq!(discovery.heads_failed, !delay.is_zero());
            assert!(!discovery.page.items[0].reconcile_current);
            if delay.is_zero() {
                assert_eq!(uncertain.head_reads.load(Ordering::SeqCst), 0);
            }
        }
        uncertain.fail_reads.store(false, Ordering::SeqCst);
        uncertain.head_timeout.store(false, Ordering::SeqCst);
        DurableAgentUpdateRecorder::new(store.clone())
            .record_update(
                &agent.id,
                3,
                &mut AgentMessageAssembly::default(),
                ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
                crate::now_rfc3339(),
            )
            .await
            .unwrap();
        assert_eq!(worker.run_scheduled(id).await.unwrap(), Duration::ZERO);
        assert_eq!(
            store.get_command(id).await.unwrap().unwrap().status,
            AgentResponseStatus::Completed
        );
        assert_eq!(worker.run_scheduled(id).await.unwrap(), Duration::ZERO);
        assert!(worker
            .discover_scheduled(&page)
            .await
            .unwrap()
            .page
            .items
            .is_empty());
    }
}
