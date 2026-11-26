use super::*;
use layer2_core::agent_ownership::OwnershipError;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn failed_inspection_parks_across_owner_takeover() {
    tokio::try_join!(
        Box::pin(observe_takeover(AgentWorkStep::CloseProcess, "auth", false)),
        Box::pin(observe_takeover(
            AgentWorkStep::PublishConnection,
            "auth",
            false
        )),
        Box::pin(observe_takeover(
            AgentWorkStep::PublishConnection,
            "confirm_ambiguous",
            false
        )),
        Box::pin(observe_takeover(AgentWorkStep::CloseProcess, "auth", true)),
        Box::pin(observe_takeover(
            AgentWorkStep::CloseProcess,
            "after_close",
            false
        )),
    )
    .unwrap();
}

async fn observe_takeover(
    step: AgentWorkStep,
    refusal: &'static str,
    interrupt_retry: bool,
) -> Result<(), OwnershipError> {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("inspection_takeover_{}", uuid::Uuid::new_v4().simple());
    let standalone = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = ready_conversation_fixture(&standalone, "inspection").await;
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
    standalone
        .save_worktree(revision, worktree.clone())
        .await
        .unwrap();
    let home = bind_runtime(&standalone, &agent, HANDLE).await;
    let sandbox = standalone
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let open = layer3_adapter::AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(&agent, &sandbox, &worktree),
        None,
    );
    let receipt = agent
        .work
        .receipts
        .iter_mut()
        .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
        .unwrap();
    assert_eq!(receipt.operation_id, open.operation_id);
    receipt.request_sha256 = open.request_sha256;
    replace_agent_fixture(&standalone, &agent).await;
    let crash_after_close = refusal == "after_close";
    let runtime = Runtime::start(if crash_after_close { "auth" } else { refusal }).await;
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
        home.clone(),
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

    if interrupt_retry {
        use crate::store::child_writer_claims::{persist, ChildWriterClaim};
        let second_authority = second_manager.acquire(&agent.id).await?.unwrap();
        let now = SystemTime::now();
        let ready = layer2_core::rfc3339_at(now);
        let expires = layer2_core::rfc3339_at(now + Duration::from_secs(18 * 60));
        let mut claim = parked.clone();
        claim.work.lease = None;
        claim
            .work
            .claim("queued".into(), &ready, expires.clone())
            .unwrap();
        claim.revision += 1;
        assert!(
            !second_authority
                .run(persist(
                    &second,
                    parked.revision,
                    ChildWriterClaim::Agent(Box::new(claim))
                ))
                .await?
                .unwrap(),
            "claim transaction itself refuses a parked step"
        );
        assert!(crate::store::agent_inspection_park::recovery::consume(
            &second.agents,
            agent.id.as_str(),
            park_id
        )
        .await
        .unwrap());
        let claimed = second_authority
            .run(second.claim_agent_work(&agent.id, "interrupted", ready, expires))
            .await?
            .unwrap()
            .unwrap();
        assert_eq!(claimed.work.step, step);
        let parent = second
            .get_sandbox(&agent.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let binding = parent.runtime.as_ref().unwrap();
        let mut changed_binding = binding.clone();
        changed_binding.boot_id = Some("b".repeat(32));
        assert_eq!(
            second_authority
                .run(second.confirm_agent_inspection(&claimed, &changed_binding))
                .await
                .unwrap(),
            Err(SandboxStoreError::Conflict)
        );
        let removed = second
            .agents
            .find_one_and_update(
                doc! {"_id": agent.id.as_str()},
                doc! {"$unset": {field: ""}},
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second_authority
                .run(second.confirm_agent_inspection(&claimed, binding))
                .await
                .unwrap(),
            Err(SandboxStoreError::Conflict)
        );
        second
            .agents
            .update_one(
                doc! {"_id": agent.id.as_str()},
                doc! {"$set": {field: removed.get_document(field).unwrap().clone()}},
            )
            .await
            .unwrap();

        assert!(second.agent_inspection_parked(&agent.id).await.unwrap());
        assert!(!crate::store::agent_inspection_park::recovery::consume(
            &second.agents,
            agent.id.as_str(),
            park_id
        )
        .await
        .unwrap());
        let second_owners = second.agent_ownership.as_ref().unwrap();
        second_owners.heartbeat("second", true).await?;
        tokio::time::timeout(Duration::from_secs(18), async {
            while second_owners.owner(&agent.id).await.unwrap().is_some() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("the interrupted worker did not release its ownership");
        let replacement = Arc::new(
            MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .enable_agent_ownership("first-restarted".into())
                .await
                .unwrap()
                .with_agent_inspection_parking(true),
        );
        let replacement_manager = OwnershipCoordinator::register(
            replacement.agent_ownership.as_ref().unwrap().clone(),
            member("first-restarted"),
        )
        .await?;
        assert!(replacement
            .agent_inspection_parked(&agent.id)
            .await
            .unwrap());
        assert!(!replacement
            .due_agent_work(&layer2_core::rfc3339_at(SystemTime::now()), 10)
            .await
            .unwrap()
            .contains(&agent.id));
        assert!(replacement_manager.owned().await.is_empty());
        let owner = raw_owner
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            owner.get_i64("epoch").unwrap(),
            second_authority.token.epoch as i64
        );
        runtime.assert_counts(1);
        client.database(&name).drop().await.unwrap();
        return Ok(());
    }

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
    if crash_after_close {
        runtime.repair();
        runtime.hold_first_close();
        let target = agent.id.clone();
        let task = tokio::spawn(async move { worker.run_one(&target, SystemTime::now()).await });
        tokio::time::timeout(Duration::from_secs(8), runtime.close_received())
            .await
            .unwrap();
        assert!(!second.agent_inspection_parked(&agent.id).await.unwrap());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        runtime.release_close();
        let second_owners = second.agent_ownership.as_ref().unwrap();
        second_owners.heartbeat("second", true).await?;
        tokio::time::timeout(Duration::from_secs(18), async {
            while second_owners.owner(&agent.id).await.unwrap().is_some() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .expect("the interrupted Close retains the original lease until expiry");
        let successor = Arc::new(
            MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .enable_agent_ownership("first-restarted".into())
                .await
                .unwrap()
                .with_agent_inspection_parking(true),
        );
        let manager = OwnershipCoordinator::register(
            successor.agent_ownership.as_ref().unwrap().clone(),
            member("first-restarted"),
        )
        .await?;
        let worker = build_worker(
            successor.clone(),
            manager,
            runtime.transport.clone(),
            home,
            None,
        );
        assert!(worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
        let advanced = successor.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(advanced.work.step, AgentWorkStep::PublishStopped);
        assert_eq!(
            advanced
                .work
                .receipts
                .iter()
                .filter(|receipt| receipt.step == step)
                .count(),
            1
        );
        runtime.assert_close_replayed();
        client.database(&name).drop().await.unwrap();
        return Ok(());
    }
    if confirmations == 0 {
        runtime.repair();
        assert!(worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
        let advanced = second.get_agent(&agent.id).await.unwrap().unwrap();
        let controls = if step == AgentWorkStep::CloseProcess {
            assert_eq!(advanced.work.step, AgentWorkStep::PublishStopped);
            assert_eq!(
                advanced
                    .work
                    .receipts
                    .iter()
                    .filter(|receipt| receipt.step == step)
                    .count(),
                1
            );
            3
        } else {
            assert_eq!(advanced.work.step, AgentWorkStep::None);
            let mut expected_conversation = parked.conversation.clone();
            expected_conversation.last_seq += 1;
            assert_eq!(advanced.conversation, expected_conversation);
            let completion = second
                .agent_log(&agent.id, parked.conversation.last_seq, 2)
                .await
                .unwrap();
            assert_eq!(completion.len(), 1);
            assert_eq!(completion[0].kind, AgentLogKind::WorkStepCompleted { step });
            2
        };
        runtime.assert_repaired_probe(controls);
        assert!(!second.agent_inspection_parked(&agent.id).await.unwrap());
    } else {
        assert!(!worker.run_one(&agent.id, SystemTime::now()).await.unwrap());
        runtime.assert_counts_with_confirmation(2, 2 * confirmations);
        let next = second
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let next = next.get_document(field).unwrap();
        assert_ne!(next.get_str("id").unwrap(), park_id);
        assert!(next.get_bool("probe_spent").unwrap());
    }
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
    println!(
        "step={step:?} unchanged_epoch={} deferred_attempts=3 reviewed_retry=1",
        original.token.epoch
    );

    client.database(&name).drop().await.unwrap();
    Ok(())
}
