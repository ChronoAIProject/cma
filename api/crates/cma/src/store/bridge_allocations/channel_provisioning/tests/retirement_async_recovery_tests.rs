use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_retirement_lost_async_close_replays_exact_intent_before_key_cleanup() {
    let lifecycle = LifecycleFixture::new().await;
    let (queued, dispatched) = support::dispatch(&lifecycle, "retirement-lost-async-close").await;
    let resources = lifecycle.f.original.resources.clone();
    let registry = lifecycle.registry.clone();
    let adapter = lifecycle.f.original.adapter.clone();
    let (stopper, first) = retirement_workers(
        resources.clone(),
        adapter.clone(),
        registry.clone(),
        "retirement-before-restart",
    );
    let f = RetirementFixture::with_agents(lifecycle.f, registry.clone(), Some(stopper));
    let id = dispatched.receipt.envelope.agent_id.clone();
    let before = f.sandbox().await;
    let original_channel_source = support::source_records(&f, &dispatched).await;
    let receipts = f.db().collection::<Document>("bridge_callback_receipts");
    let original_receipt = receipts
        .find_one(doc! {"_id":&queued.inbox.id})
        .await
        .unwrap()
        .unwrap();
    let original_log = support::log(&f).await;
    f.start().await;
    f.step().await;
    f.step().await;
    assert!(first.run_one(&id, SystemTime::now()).await.unwrap());
    f.f.original
        .backend
        .lose_next_response(WireOperation::CloseAgent);
    assert!(first.run_one(&id, SystemTime::now()).await.unwrap());
    let unknown = resources.get_agent(&id).await.unwrap().unwrap();
    assert!(unknown.status.is_live());
    assert_eq!(unknown.work.step, AgentWorkStep::CloseProcess);
    assert_eq!(unknown.work.attempts, 1);
    assert!(unknown.work.lease.is_none());
    assert!(unknown
        .work
        .receipts
        .iter()
        .all(|receipt| receipt.step != AgentWorkStep::CloseProcess));
    let requests = support::close_requests(&f).await;
    assert_eq!(requests.len(), 1);
    f.retry().await;
    f.step().await;
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::StopIntent
    );
    assert_eq!(f.sandbox().await.credential, before.credential);
    assert_eq!(f.sandbox().await.runtime, before.runtime);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
    drop(first);
    let reopened = Arc::new(
        crate::store::MongoSandboxStore::open(&f.f.original.client, &f.f.original.database)
            .await
            .unwrap(),
    );
    let (_, restarted) = retirement_workers(
        reopened.clone(),
        adapter,
        Arc::new(crate::sandbox::LiveAgentRegistry::default()),
        "retirement-after-restart",
    );
    support::await_agent_work_due(&reopened, &id).await;
    assert!(restarted.run_one(&id, SystemTime::now()).await.unwrap());
    let closed = reopened.get_agent(&id).await.unwrap().unwrap();
    assert_eq!(closed.work.step, AgentWorkStep::PublishStopped);
    assert_eq!(
        closed.conversation.conversation_id,
        unknown.conversation.conversation_id
    );
    assert_eq!(closed.creation_fingerprint, unknown.creation_fingerprint);
    let replay = support::close_requests(&f).await;
    assert_eq!(replay.len(), 2);
    assert_eq!(replay[0].request, replay[1].request);
    assert!(replay[1].authority_fence > replay[0].authority_fence);
    let receipt = closed
        .work
        .receipts
        .iter()
        .find(|receipt| receipt.step == AgentWorkStep::CloseProcess)
        .unwrap();
    assert_eq!(receipt.operation_id, replay[0].request.operation_id);
    assert_eq!(receipt.request_sha256, replay[0].request.request_sha256);
    f.retry().await;
    f.step().await;
    assert_eq!(
        f.receipt().await.unwrap().phase,
        RetirementPhase::StopIntent
    );
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
    assert!(restarted.run_one(&id, SystemTime::now()).await.unwrap());
    assert_eq!(
        reopened.get_agent(&id).await.unwrap().unwrap().status,
        AgentStatus::Stopped
    );
    let commands = AgentCommandWorker::new(
        reopened.clone(),
        Arc::new(support::NoDispatch),
        "retirement-lost-close-recovery".into(),
    )
    .unwrap();
    let command_id = &dispatched.receipt.envelope.command_id;
    commands.run_one(command_id).await.unwrap();
    let recovered = reopened.get_command(command_id).await.unwrap().unwrap();
    assert_eq!(recovered.status, AgentResponseStatus::Failed);
    assert_eq!(
        recovered.work.failure.as_ref().unwrap().code,
        CommandFailureCode::RuntimeLost
    );
    assert_eq!(recovered.receipt, dispatched.receipt);
    assert_eq!(recovered.work.dispatch, dispatched.work.dispatch);
    commands.run_one(command_id).await.unwrap();
    assert_eq!(
        reopened.get_command(command_id).await.unwrap().unwrap(),
        recovered
    );
    assert_eq!(
        support::source_records(&f, &dispatched).await,
        original_channel_source
    );
    assert_eq!(
        receipts
            .find_one(doc! {"_id":&queued.inbox.id})
            .await
            .unwrap()
            .unwrap(),
        original_receipt
    );
    assert!(support::log(&f).await.starts_with(&original_log));
    f.retry().await;
    f.step().await;
    assert_eq!(f.receipt().await.unwrap().phase, RetirementPhase::Revoke);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
    f.step().await;
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert_eq!(support::close_requests(&f).await.len(), 2);
    assert_eq!(f.adapter.teardowns.load(Ordering::SeqCst), 0);
    assert_eq!(f.stops.calls.load(Ordering::SeqCst), 0);
    f.close().await;
}
