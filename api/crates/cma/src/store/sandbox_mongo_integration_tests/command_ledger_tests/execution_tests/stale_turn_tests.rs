use mongodb::bson::{doc, DateTime};

use super::*;
use crate::{
    sandbox::LiveAgentRegistry,
    store::stale_turns::{Candidate, Observation, ProofBudget, StaleTurnStore},
};

mod native_stop_trace;
pub(super) mod settlement;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_scan_holds_a_silent_dispatch_and_the_recorder_completes_it_once() {
    use layer3_adapter::{ConversationUpdate, PromptResponse, TurnStopReason};

    let (client, name) = database("stale_turn").await;
    let input = ConversationInputSender::spawn(2, |_| async { Ok(()) });
    let mut parent = active_sandbox("sbx_stale_turn");
    parent.lifecycle.home = Some(Box::new(
        crate::store::sandbox_unpublished_recovery::mongo_integration_tests::home(&parent.id),
    ));
    let runtime = parent.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    runtime.boot_id = Some("b".repeat(32));
    parent.validate().unwrap();
    let (store, agent, parent, execution, _, sends) = composed_with_input_history(
        &client,
        &name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        parent,
        None,
        Some((input.clone(), "synthetic-conversation".into())),
    )
    .await;
    let registry = Arc::new(LiveAgentRegistry::default());
    let (output, receiver) = tokio::sync::mpsc::channel(4);
    let attachment = registry
        .attach(
            agent.id.clone(),
            agent.conversation.conversation_id.clone().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            1,
            agent.capabilities.clone(),
            input,
            receiver,
            DurableAgentUpdateRecorder::new(store.clone()),
        )
        .unwrap();
    registry.publish_and_activate(&attachment).unwrap();
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "stale-turn"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    let worker = AgentCommandWorker::new(store.clone(), execution, "stale-scan".into()).unwrap();
    worker.run_one(id).await.unwrap();
    store.reconcile_command(id).await.unwrap();
    let before = store.get_agent(&agent.id).await.unwrap().unwrap();
    let scan = StaleTurnStore::new(store.clone());
    let lease = scan.claim().await.unwrap().unwrap();
    let page = scan.page(&lease).await.unwrap();
    let row = page
        .rows
        .iter()
        .find(|row| row.get_str("_id").ok() == Some(id.as_str()))
        .unwrap();
    assert!(Candidate::from_row(row, DateTime::now()).is_none());
    let future = DateTime::from_millis(DateTime::now().timestamp_millis() + 16 * 60 * 1000);
    let candidate = Candidate::from_row(row, future).unwrap();
    match scan
        .observe(&candidate, future, &mut ProofBudget::default())
        .await
        .unwrap()
    {
        Observation::Quiet { target, age, .. } => {
            assert_eq!(target.agent_id, agent.id);
            assert!(age >= 15 * 60);
        }
        Observation::Hold(reason) => panic!("expected quiet observation, got {reason}"),
        Observation::Progressing => panic!("silent turn has no new progress"),
    }
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), before);
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    output
        .send(ConversationUpdate::TurnEnded(PromptResponse::new(
            TurnStopReason::EndTurn,
        )))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.reconcile_command(id).await.unwrap().status != AgentResponseStatus::Completed {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let after = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(after.status, AgentStatus::Ready);
    assert_eq!(after.conversation.active_turn, None);
    assert!(matches!(
        scan.observe(&candidate, future, &mut ProofBudget::default())
            .await
            .unwrap(),
        Observation::Hold("canonical_result")
    ));
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    drop(output);
    drop(registry);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_scan_resumes_at_last_examined_row_and_fences_previous_holder() {
    let (client, name) = database("stale_turn_cursor").await;
    let (store, _, _, _, _, _) = composed(&client, &name).await;
    let other_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let other_store = Arc::new(MongoSandboxStore::open(&other_client, &name).await.unwrap());
    let first = StaleTurnStore::new(store.clone());
    let second = StaleTurnStore::new(other_store);
    let state = client
        .database(&name)
        .collection::<mongodb::bson::Document>(crate::store::agent_ownership::OWNERS);
    let command_row = |id: String, status: &str, order: i32| {
        doc! {"_id": &id, "status": status, "agent_id": "agt_scan", "agent_order": order,
        "scope": "scan_fixture", "key_sha256": &id}
    };
    store
        .agent_commands
        .insert_many(
            (0..150).map(|index| command_row(format!("cmd_{index:04}"), "in_progress", index)),
        )
        .await
        .unwrap();
    store
        .agent_commands
        .insert_one(command_row("cmd_wait".into(), "waiting_action", 150))
        .await
        .unwrap();

    let lease = first.claim().await.unwrap().unwrap();
    assert!(second.claim().await.unwrap().is_none());
    let page = first.page(&lease).await.unwrap();
    assert_eq!(page.rows.len(), 128);
    assert!(!page.eof);
    assert!(first
        .advance(&lease, Some("cmd_0003"), false)
        .await
        .unwrap());
    assert!(!first
        .advance(&lease, Some("cmd_0010"), false)
        .await
        .unwrap());
    assert!(second.claim().await.unwrap().is_none());
    state
        .update_one(
            doc! {},
            doc! {"$set": {"scan_expires_at": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let next = second.claim().await.unwrap().unwrap();
    let resumed = second.page(&next).await.unwrap();
    assert_eq!(resumed.rows[0].get_str("_id").unwrap(), "cmd_0004");
    assert!(!first
        .advance(&lease, Some("cmd_0000"), false)
        .await
        .unwrap());
    assert!(second
        .advance(&next, Some("cmd_0131"), false)
        .await
        .unwrap());

    state
        .update_one(
            doc! {},
            doc! {"$set": {"scan_expires_at": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let tail = first.claim().await.unwrap().unwrap();
    let page = first.page(&tail).await.unwrap();
    assert!(page.eof);
    assert_eq!(page.rows.len(), 18);
    assert!(first.advance(&tail, Some("cmd_0149"), true).await.unwrap());
    state
        .update_one(
            doc! {},
            doc! {"$set": {"scan_expires_at": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let waiting = second.claim().await.unwrap().unwrap();
    let page = second.page(&waiting).await.unwrap();
    assert!(page.eof);
    assert_eq!(page.rows[0].get_str("_id").unwrap(), "cmd_wait");
    assert!(second
        .advance(&waiting, Some("cmd_wait"), true)
        .await
        .unwrap());
    state
        .update_one(
            doc! {},
            doc! {"$set": {"scan_expires_at": DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let wrapped = first.claim().await.unwrap().unwrap();
    assert_eq!(
        first.page(&wrapped).await.unwrap().rows[0]
            .get_str("_id")
            .unwrap(),
        "cmd_0000"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stale_turn_scan_cursor_allows_replica_ownership_startup() {
    use crate::store::agent_ownership::MongoAgentOwnership;
    use layer2_core::agent_ownership::{
        Member, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS, PROTOCOL_VERSION,
    };

    let (client, name) = database("stale_turn_restart").await;
    let (store, agent, _, _, _, _) = composed(&client, &name).await;
    let ownership = MongoAgentOwnership::open(&client, &name).await.unwrap();
    let instance = "replica-before-restart";
    ownership
        .register(Member {
            instance_id: instance.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
        })
        .await
        .unwrap();
    let owner = ownership
        .acquire(&agent.id, instance)
        .await
        .unwrap()
        .unwrap();

    let scan = StaleTurnStore::new(store);
    let lease = scan.claim().await.unwrap().unwrap();
    let restarted_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let restarted = MongoAgentOwnership::open(&restarted_client, &name)
        .await
        .unwrap();
    assert_eq!(
        restarted.owner(&agent.id).await.unwrap(),
        Some(owner.clone())
    );

    assert!(scan
        .advance(&lease, Some("cmd_examined"), false)
        .await
        .unwrap());
    let after_advance = MongoAgentOwnership::open(&restarted_client, &name)
        .await
        .unwrap();
    assert_eq!(after_advance.owner(&agent.id).await.unwrap(), Some(owner));
    client.database(&name).drop().await.unwrap();
}
