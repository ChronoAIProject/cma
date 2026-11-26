#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI and CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn media_continuation_unknown_requeue_commit_recovers_same_slot_after_source_outage() {
    let (f, claimed) = deferred_fixture("media_unknown_requeue").await;
    let app = format!("media-requeue-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let retry_store = MongoSandboxStore::open(&client, &f.name).await.unwrap();
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    admin.database("admin").run_command(doc!{"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":{"failCommands":["commitTransaction"],"appName":app,"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected media retry ambiguity"}}}).await.unwrap();
    let result = retry_store
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(CommandLedgerError::Unavailable)));
    crate::store::test_visibility::majority_barrier(&admin, &f.name).await;
    let reopened = MongoSandboxStore::open(&f.client, &f.name).await.unwrap();
    let queued = reopened
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    assert_eq!(queued.receipt, claimed.receipt);
    assert_eq!(queued.work.deferred_attempts.len(), 1);
    assert_eq!(
        queued.work.deferred_attempts[0].dispatch.claim,
        claimed.work.lease.as_ref().unwrap().claim
    );
    assert!(queued.work.dispatch.is_none());
    assert!(matches!(
        f.service.check(&queued.receipt.envelope).await,
        Err(CommandAuthorizationError::Unavailable)
    ));
    assert_eq!(
        reopened
            .reconcile_command(&queued.receipt.envelope.command_id)
            .await
            .unwrap(),
        queued
    );
    if !queued
        .continuation_retry_delay(&layer2_core::now_rfc3339())
        .is_zero()
    {
        assert!(f
            .store
            .claim_next_command(claim_request(
                &f.agent,
                "early-retry",
                &layer2_core::now_rfc3339()
            ))
            .await
            .unwrap()
            .is_none());
    }
    f.peer.outage.store(false, Ordering::SeqCst);
    let receipt = f
        .service
        .attachment(f.caller.clone(), f.path.clone())
        .await
        .unwrap();
    let later = f
        .service
        .supplement(f.caller.clone(), f.path.clone(), f.supplement(&receipt, ""))
        .await
        .unwrap();
    assert!(later.command.receipt.agent_order > queued.receipt.agent_order);
    // A later artifact must not overtake this exact original queue position.
    assert!(f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "fifo-during-backoff",
            &queued.work.deferred_attempts[0].failed_at
        ))
        .await
        .unwrap()
        .is_none());
    tokio::time::sleep(queued.continuation_retry_delay(&layer2_core::now_rfc3339())).await;
    let retry = f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "recovered-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let authority = f
        .auth
        .authorize_resource_command(&f.access, &retry.receipt.envelope)
        .await
        .unwrap()
        .with_source_guard(f.service.clone());
    let (runtime, sends) = runtime(&f, usize::MAX, false);
    AgentOperationService::new(f.store.clone(), runtime)
        .execute_resource_claim(&authority, &retry)
        .await
        .unwrap();
    let saved = f
        .store
        .reconcile_command(&retry.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(saved.receipt, claimed.receipt);
    assert_eq!(saved.work.deferred_attempts, queued.work.deferred_attempts);
    assert_eq!(saved.status, AgentResponseStatus::InProgress);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_continuation_cancellation_racing_retry_claim_is_not_resurrected() {
    let (f, claimed) = deferred_fixture("media_retry_cancel").await;
    let queued = f
        .store
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap();
    let submission = QueuedCancellationSubmission {
        agent_id: f.agent.id.clone(),
        target: QueuedCancellationTarget {
            response_id: queued.receipt.response_id.clone().unwrap(),
            source_command_id: queued.receipt.envelope.command_id.clone(),
            expected_command_revision: queued.revision,
        },
        idempotency_key: "cancel-pending-media".into(),
    };
    tokio::time::sleep(queued.continuation_retry_delay(&layer2_core::now_rfc3339())).await;
    let (cancel, retry) = tokio::join!(
        f.commands.cancel_queued_response(
            &f.caller.context,
            f.caller.credential.as_ref().unwrap().value(),
            CommandSubmissionSource::Api,
            submission.clone()
        ),
        f.store.claim_next_command(claim_request(
            &f.agent,
            "racing-retry",
            &layer2_core::now_rfc3339()
        ))
    );
    let retry = retry.unwrap();
    if let Some(retry) = retry {
        assert!(
            cancel.is_err(),
            "a live retry claim and queued cancellation cannot both win"
        );
        // No IO was attempted. Release the known claim, then explicitly cancel
        // the freshly observed revision; this does not clear a dispatch.
        f.store
            .release_command_claim(
                &retry.work.lease.as_ref().unwrap().claim,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        let current = f
            .store
            .get_command(&queued.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        let mut next = submission;
        next.target.expected_command_revision = current.revision;
        next.idempotency_key = "cancel-after-retry-claim-release".into();
        f.commands
            .cancel_queued_response(
                &f.caller.context,
                f.caller.credential.as_ref().unwrap().value(),
                CommandSubmissionSource::Api,
                next,
            )
            .await
            .unwrap();
    } else {
        cancel.unwrap();
    }
    f.peer.outage.store(false, Ordering::SeqCst);
    let saved = f
        .store
        .reconcile_command(&queued.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(saved.status, AgentResponseStatus::Cancelled);
    assert_eq!(saved.receipt, queued.receipt);
    assert_eq!(saved.work.deferred_attempts, queued.work.deferred_attempts);
    assert!(f
        .store
        .claim_next_command(claim_request(
            &f.agent,
            "cancelled-retry",
            &layer2_core::now_rfc3339()
        ))
        .await
        .unwrap()
        .is_none());
    f.close().await;
}
