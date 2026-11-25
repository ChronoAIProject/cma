use super::*;
use layer2_core::agent_ownership::OwnershipError;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn failed_inspection_parks_across_owner_takeover() {
    tokio::try_join!(
        observe_takeover(AgentWorkStep::CloseProcess, "auth"),
        observe_takeover(AgentWorkStep::PublishConnection, "auth"),
        observe_takeover(AgentWorkStep::PublishConnection, "confirm_ambiguous"),
    )
    .unwrap();
}

async fn observe_takeover(
    step: AgentWorkStep,
    refusal: &'static str,
) -> Result<(), OwnershipError> {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("inspection_takeover_{}", uuid::Uuid::new_v4().simple());
    let standalone = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&standalone, "inspection").await;
    let workspace = standalone
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut worktree = standalone
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let revision = worktree.revision;
    worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
    worktree.base_commit = workspace.resolved_commit.unwrap();
    worktree.revision += 1;
    standalone.save_worktree(revision, worktree).await.unwrap();
    let home = bind_runtime(&standalone, &agent, HANDLE).await;
    let runtime = Runtime::start(refusal).await;
    let confirmations = usize::from(refusal == "confirm_ambiguous");
    if step == AgentWorkStep::CloseProcess {
        standalone
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason: AgentStopReason::Owner,
                requested_at: layer2_core::rfc3339_at(SystemTime::now()),
            })
            .await
            .unwrap();
    }

    let first = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .enable_agent_ownership("first".into())
            .await
            .unwrap()
            .with_agent_inspection_parking(true),
    );
    let owners = first.agent_ownership.as_ref().unwrap();
    let first_manager = OwnershipCoordinator::register(owners.clone(), member("first")).await?;
    let worker = build_worker(
        first.clone(),
        first_manager.clone(),
        runtime.transport.clone(),
        home.clone(),
        None,
    );
    if step == AgentWorkStep::CloseProcess {
        assert!(worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
    }
    let before = first.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(before.work.step, step);
    assert!(!worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
    runtime.assert_counts_with_confirmation(1, confirmations);
    let authority = first_manager.owned().await.pop().unwrap();
    assert!(authority.receiver_cancelled());
    assert!(authority.revoke_transports().await.is_err());
    let original = owners.owner(&agent.id).await?.unwrap();
    assert_eq!(original.token, authority.token);

    let parked = first.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(parked.work.step, before.work.step);
    assert_eq!(parked.work.attempts, before.work.attempts);
    assert_eq!(parked.work.receipts, before.work.receipts);
    assert_eq!(parked.conversation, before.conversation);
    assert_eq!(parked.status, before.status);
    assert!(parked.work.lease.is_some());
    let field = crate::store::agent_inspection_park::FIELD;
    let row = first
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let marker = row.get_document(field).unwrap();
    if confirmations == 0 {
        assert_eq!(marker.get_str("phase").unwrap(), "challenge");
        assert_eq!(marker.get_str("reason").unwrap(), "http_auth_refused");
        assert_eq!(marker.get_i32("http_status").unwrap(), 403);
    } else {
        assert_eq!(marker.get_str("phase").unwrap(), "confirm");
    }
    let park_id = marker.get_str("id").unwrap();
    owners.heartbeat("first", true).await?;
    tokio::time::timeout(Duration::from_secs(18), async {
        while owners.owner(&agent.id).await.unwrap().is_some() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the failed receiver never acknowledged release");

    let second = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .enable_agent_ownership("second".into())
            .await
            .unwrap()
            .with_agent_inspection_parking(true),
    );
    let second_manager = OwnershipCoordinator::register(
        second.agent_ownership.as_ref().unwrap().clone(),
        member("second"),
    )
    .await?;
    let worker = build_worker(
        second.clone(),
        second_manager.clone(),
        runtime.transport.clone(),
        home,
        None,
    );
    let healthy = ready_conversation_fixture(&standalone, "healthy").await;
    for _ in 0..3 {
        let due = second
            .due_agent_work(&layer2_core::rfc3339_at(SystemTime::now()), 10)
            .await
            .unwrap();
        assert!(due.contains(&healthy.id));
        assert!(!due.contains(&agent.id));
        assert!(!worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
    }
    runtime.assert_counts_with_confirmation(1, confirmations);
    assert!(second_manager.owned().await.is_empty());
    let raw_owner = client
        .database(&name)
        .collection::<mongodb::bson::Document>("cma_agent_manager_owners");
    let current = raw_owner
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current.get_i64("epoch").unwrap(),
        original.token.epoch as i64
    );
    assert_eq!(second.get_agent(&agent.id).await.unwrap().unwrap(), parked);

    use crate::store::agent_inspection_park::recovery::consume;
    assert!(
        !consume(&second.agents, agent.id.as_str(), "expired-review")
            .await
            .unwrap()
    );
    assert!(consume(&second.agents, agent.id.as_str(), park_id)
        .await
        .unwrap());
    assert!(!consume(&second.agents, agent.id.as_str(), park_id)
        .await
        .unwrap());
    assert!(!worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
    runtime.assert_counts_with_confirmation(2, 2 * confirmations);
    let next = second
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next.get_i64("agent_manager_work_epoch").unwrap(),
        original.token.epoch as i64 + 1
    );
    assert_ne!(
        next.get_document(field).unwrap().get_str("id").unwrap(),
        park_id
    );
    println!(
        "step={step:?} unchanged_epoch={} deferred_attempts=3 reviewed_retry=1",
        original.token.epoch
    );

    client.database(&name).drop().await.unwrap();
    Ok(())
}
