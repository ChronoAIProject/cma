use super::*;
use layer2_core::agent_ownership::RenewalBatch;
use mongodb::options::ClientOptions;

const INSTANCE: &str = "batch-member";

async fn owners(store: &MongoAgentOwnership) -> [OwnershipToken; 2] {
    store.register(member(INSTANCE)).await.unwrap();
    let mut tokens = Vec::new();
    for id in ["batch-agent-a", "batch-agent-b"] {
        tokens.push(
            store
                .acquire(&AgentId(id.into()), INSTANCE)
                .await
                .unwrap()
                .unwrap()
                .token,
        );
    }
    tokens.try_into().unwrap()
}

async fn snapshots(
    store: &MongoAgentOwnership,
    tokens: &[OwnershipToken],
) -> (Document, Vec<Document>) {
    let member = store
        .members
        .find_one(doc! {"_id": INSTANCE})
        .await
        .unwrap()
        .unwrap();
    let mut records = Vec::new();
    for token in tokens {
        records.push(
            store
                .owners
                .find_one(doc! {"_id": token.agent_id.as_str()})
                .await
                .unwrap()
                .unwrap(),
        );
    }
    (member, records)
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn batch_renewal_partial_match_preserves_original_records_and_exact_epochs() {
    let store = fixture().await;
    let tokens = owners(&store).await;
    let original = snapshots(&store, &tokens).await;
    let mut mixed_epochs = tokens.clone();
    mixed_epochs[1].epoch += 1;
    assert_eq!(
        store.renew_batch(&mixed_epochs).await,
        Ok(RenewalBatch::PartialMatchAborted)
    );
    assert_eq!(snapshots(&store, &tokens).await, original);

    // Independent ID and epoch sets would renew both of these records.
    store
        .owners
        .update_one(
            doc! {"_id": tokens[1].agent_id.as_str()},
            doc! {"$inc": {"epoch": 1_i64}},
        )
        .await
        .unwrap();
    let revised = snapshots(&store, &tokens).await;
    mixed_epochs[0].epoch += 1;
    mixed_epochs[1].epoch -= 1;
    assert_eq!(
        store.renew_batch(&mixed_epochs).await,
        Ok(RenewalBatch::PartialMatchAborted)
    );
    assert_eq!(snapshots(&store, &tokens).await, revised);

    assert!(matches!(
        store
            .renew_batch(&[tokens[0].clone(), tokens[0].clone()])
            .await,
        Err(OwnershipError::Invalid(_))
    ));
    let exact = [
        tokens[0].clone(),
        OwnershipToken {
            epoch: tokens[1].epoch + 1,
            ..tokens[1].clone()
        },
    ];
    let RenewalBatch::Committed(committed) = store.renew_batch(&exact).await.unwrap() else {
        panic!("current tokens must renew")
    };
    assert_eq!(
        committed
            .iter()
            .map(|lease| lease.token.clone())
            .collect::<Vec<_>>(),
        exact
    );
    let renewed = snapshots(&store, &exact).await;
    assert_eq!(
        renewed.0.get_i64("renewal_revision").unwrap(),
        revised.0.get_i64("renewal_revision").unwrap() + 1
    );
    assert_eq!(
        renewed.0.get_i64("owned_agents").unwrap(),
        revised.0.get_i64("owned_agents").unwrap()
    );
    assert_eq!(store.check(&tokens[1]).await, Err(OwnershipError::Stale));
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn batch_renewal_rolls_back_when_a_peer_takes_over_an_expired_token() {
    let store = fixture().await;
    let tokens = owners(&store).await;
    let peer = "batch-peer-0";
    store.register(member(peer)).await.unwrap();
    assert_eq!(preferred(&store, &tokens[0].agent_id).await, peer);
    expire(&store, &tokens[0]).await;

    let app = format!("batch-takeover-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app.clone());
    let faulted = MongoAgentOwnership::open(
        &Client::with_options(options).unwrap(),
        store.database.name(),
    )
    .await
    .unwrap();
    let admin = store.client.database("admin");
    // Pause the member write after the batch has opened its snapshot.
    let configured = admin.run_command(doc! {"configureFailPoint": "failCommand", "mode": {"times": 1_i32}, "data": {
        "appName": app, "failCommands": ["update"], "blockConnection": true, "blockTimeMS": 900_i32,
    }}).await.unwrap();
    let count = configured
        .get_i64("count")
        .or_else(|_| configured.get_i32("count").map(i64::from))
        .unwrap();
    let batch = tokio::spawn({
        let tokens = tokens.clone();
        async move { faulted.renew_batch(&tokens).await }
    });
    admin
        .run_command(doc! {"waitForFailPoint": "failCommand", "timesEntered": count + 1, "maxTimeMS": 1500_i32})
        .await
        .unwrap();
    let takeover = store
        .acquire(&tokens[0].agent_id, peer)
        .await
        .unwrap()
        .unwrap();
    assert!(!batch.is_finished());
    assert_eq!(takeover.token.epoch, tokens[0].epoch + 1);
    let after_takeover = snapshots(&store, &tokens).await;
    assert_eq!(batch.await.unwrap(), Ok(RenewalBatch::PartialMatchAborted));
    assert_eq!(snapshots(&store, &tokens).await, after_takeover);

    let mut effect = store.client.start_session().await.unwrap();
    start_transaction(&mut effect).await.unwrap();
    assert_eq!(
        store.fence_transaction(&tokens[0], &mut effect).await,
        Err(SandboxStoreError::Conflict)
    );
    effect.abort_transaction().await.unwrap();
    start_transaction(&mut effect).await.unwrap();
    store
        .fence_transaction(&takeover.token, &mut effect)
        .await
        .unwrap();
    assert!(matches!(
        commit_transaction(&mut effect).await,
        CommitOutcome::Committed
    ));
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn batch_renewal_refuses_member_drain_while_a_peer_keeps_its_fence() {
    let store = fixture().await;
    let tokens = owners(&store).await;
    store.register(member("batch-peer")).await.unwrap();
    let membership = store.membership().await.unwrap();
    let peer_id = (0..100)
        .map(|index| AgentId(format!("peer-{index}")))
        .find(|id| candidates(id, &membership, None).unwrap()[0].instance_id == "batch-peer")
        .unwrap();
    let peer = store
        .acquire(&peer_id, "batch-peer")
        .await
        .unwrap()
        .unwrap();

    let mut draining = store.client.start_session().await.unwrap();
    start_transaction(&mut draining).await.unwrap();
    store
        .members
        .update_one(doc! {"_id": INSTANCE}, doc! {"$set": {"draining": true}})
        .session(&mut draining)
        .await
        .unwrap();
    let (result, committed) = tokio::join!(store.renew_batch(&tokens), async {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(store.renew(&peer.token).await.is_ok());
        commit_transaction(&mut draining).await
    });
    assert!(matches!(committed, CommitOutcome::Committed));
    assert_eq!(result, Err(OwnershipError::Stale));
    let mut effect = store.client.start_session().await.unwrap();
    start_transaction(&mut effect).await.unwrap();
    assert!(matches!(
        store.fence_transaction_typed(&tokens[0], &mut effect).await,
        Err(FenceError::Domain(SandboxStoreError::Conflict)),
    ));
    effect.abort_transaction().await.unwrap();
    assert!(store.check(&peer.token).await.is_ok());
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn batch_renewal_retries_known_abort_but_returns_no_uncertain_commit() {
    let store = fixture().await;
    let tokens = owners(&store).await;
    let initial = snapshots(&store, &tokens).await;
    let app = format!("batch-renewal-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app.clone());
    let faulted = MongoAgentOwnership::open(
        &Client::with_options(options).unwrap(),
        store.database.name(),
    )
    .await
    .unwrap();
    let admin = store.client.database("admin");
    admin
        .run_command(
            doc! {"configureFailPoint": "failCommand", "mode": {"times": 1}, "data": {
                "appName": &app, "failCommands": ["update"], "errorCode": 112,
                "errorLabels": ["TransientTransactionError"],
            }},
        )
        .await
        .unwrap();
    let recovered = faulted.renew_batch(&tokens).await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(recovered, Ok(RenewalBatch::Committed(_))));
    let renewed = snapshots(&store, &tokens).await;
    assert_eq!(
        renewed.0.get_i64("renewal_revision").unwrap(),
        initial.0.get_i64("renewal_revision").unwrap() + 1
    );

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    admin.run_command(doc! {"configureFailPoint": "failCommand", "mode": {"times": 5}, "data": {
        "appName": app, "failCommands": ["commitTransaction"],
        "writeConcernError": {"code": 50, "codeName": "MaxTimeMSExpired", "errmsg": "synthetic ambiguous batch commit"},
    }}).await.unwrap();
    let ambiguous = faulted.renew_batch(&tokens).await;
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert!(matches!(ambiguous, Err(OwnershipError::Unavailable(_))));
    crate::store::test_visibility::majority_barrier(&store.client, store.database.name()).await;
    let observed = snapshots(&store, &tokens).await;
    // An ambiguous reply does not establish whether Mongo committed. It
    // must not cause a new transaction or a partially renewed owner set.
    let before = renewed.0.get_i64("renewal_revision").unwrap();
    let after = observed.0.get_i64("renewal_revision").unwrap();
    assert!((before..=before + 1).contains(&after));
    for (current, original) in observed.1.iter().zip(&renewed.1) {
        assert_eq!(current == original, after == before);
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn batch_renewal_withheld_commit_reply_cancels_only_the_captured_authorities() {
    use super::super::mongo_renewal_faults::{Fault, RoutedStore};
    use layer2_core::agent_ownership::OwnershipCoordinator;
    use std::sync::{atomic::Ordering, Arc};

    let store = Arc::new(fixture().await);
    let agent = AgentId("withheld-batch-agent".into());
    let routed = RoutedStore::new(store.clone(), agent.clone()).await;
    let coordinator =
        OwnershipCoordinator::prepare_with_options(routed.clone(), member(INSTANCE), false, true);
    coordinator.activate().await.unwrap();
    let owner = coordinator.acquire(&agent).await.unwrap().unwrap();
    let before = store.owner(&agent).await.unwrap().unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    routed.arm(Fault::WithheldReply).await;
    coordinator.maintain().await.unwrap();
    let after = store.owner(&agent).await.unwrap().unwrap();
    assert_eq!(after.token, owner.token);
    assert!(after.expires_at_ms > before.expires_at_ms);
    assert!(owner.receiver_cancelled());
    assert!(coordinator.is_ready());
    assert_eq!(routed.calls.load(Ordering::SeqCst), 1);
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn batch_renewal_cancellation_releases_the_member_fence_without_writes() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use std::{sync::Arc, time::Duration};

    let store = fixture().await;
    let tokens = owners(&store).await;
    let original = snapshots(&store, &tokens).await;
    let app = format!("batch-cancellation-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let readback = Arc::new(tokio::sync::Notify::new());
    let observed = readback.clone();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some(OWNERS)
                && event
                    .command
                    .get_document("filter")
                    .is_ok_and(|filter| filter.contains_key("$or"))
            {
                observed.notify_one();
            }
        }
    }));
    let faulted = MongoAgentOwnership::open(
        &Client::with_options(options).unwrap(),
        store.database.name(),
    )
    .await
    .unwrap();
    let admin = store.client.database("admin");
    admin.run_command(doc! {"configureFailPoint": "failCommand", "mode": {"skip": 1}, "data": {
        "appName": app, "failCommands": ["find"], "blockConnection": true, "blockTimeMS": 350,
    }}).await.unwrap();
    let attempt = tokio::spawn({
        let tokens = tokens.clone();
        async move { faulted.renew_batch(&tokens).await }
    });
    tokio::time::timeout(Duration::from_secs(1), readback.notified())
        .await
        .unwrap();
    attempt.abort();
    assert!(matches!(attempt.await, Err(error) if error.is_cancelled()));
    admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    assert_eq!(snapshots(&store, &tokens).await, original);
    // A fresh transaction acquires the shared member write fence despite the
    // cancelled readback. The cancelled chunk did not commit its revisions.
    assert!(matches!(
        store.renew_batch(&tokens).await,
        Ok(RenewalBatch::Committed(_))
    ));
    let after = snapshots(&store, &tokens).await;
    assert_eq!(
        after.0.get_i64("renewal_revision").unwrap(),
        original.0.get_i64("renewal_revision").unwrap() + 1
    );
    store.database.drop().await.unwrap();
}
