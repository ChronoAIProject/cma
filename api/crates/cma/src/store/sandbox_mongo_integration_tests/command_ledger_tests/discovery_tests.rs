use super::work_tests::{claim_request, live_request, message_mutation};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn discovery_pages_nonterminal_records_by_id_and_checks_hidden_lookahead() {
    let (client, name) = database("command_discovery").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let mut ids = Vec::new();
    for key in ["a", "b", "c"] {
        let mut reservation = live_request(&agent, &parent, key);
        reservation.queue_capacity = 3;
        ids.push(
            record(store.reserve_command(reservation).await.unwrap())
                .receipt
                .envelope
                .command_id,
        );
    }
    ids.sort();
    let page = store
        .command_work_page(&PageRequest::new(1, None).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items[0].receipt.envelope.command_id, ids[0]);
    assert_eq!(page.next.as_deref(), Some(ids[0].as_str()));
    let next = store
        .command_work_page(&PageRequest::new(1, page.next).unwrap())
        .await
        .unwrap();
    assert_eq!(next.items[0].receipt.envelope.command_id, ids[1]);
    let last = store
        .command_work_page(&PageRequest::new(1, next.next).unwrap())
        .await
        .unwrap();
    assert_eq!(last.items[0].receipt.envelope.command_id, ids[2]);
    assert!(last.next.is_none());
    let indexes: Vec<_> = store
        .agent_commands
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(indexes.iter().any(|index| index
        .options
        .as_ref()
        .and_then(|options| options.name.as_deref())
        == Some("agent_command_work_scan")
        && index.keys == doc! { "status": 1, "_id": 1 }));
    store
        .agent_commands
        .update_one(
            doc! { "_id": ids[1].as_str() },
            doc! { "$set": { "record.receipt.envelope.request_fingerprint": "corrupt" } },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .command_work_page(&PageRequest::new(1, None).unwrap())
            .await,
        Err(CommandLedgerError::Corrupt)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_unclaimed_refusal_and_lane_claim_cannot_both_win() {
    let (client, name) = database("command_refusal_race").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let reservation = live_request(&agent, &parent, "one");
    let admitted = record(one.reserve_command(reservation.clone()).await.unwrap());
    let id = &admitted.receipt.envelope.command_id;
    let now = layer2_core::now_rfc3339();
    let (refused, claimed) = tokio::join!(
        one.reject_unclaimed_command(
            id,
            admitted.revision,
            CommandFailureCode::AuthorityRevoked,
            now.clone()
        ),
        two.claim_next_command(claim_request(&agent, "replica", &now)),
    );
    let claimed = claimed.unwrap();
    assert_ne!(refused.is_ok(), claimed.is_some());
    let current = one.get_command(id).await.unwrap().unwrap();
    if refused.is_ok() {
        assert_eq!(current.status, AgentResponseStatus::Failed);
        assert!(current.work.lease.is_none());
        assert!(one
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    } else {
        assert_eq!(refused, Err(CommandLedgerError::StaleClaim));
        assert_eq!(current.status, AgentResponseStatus::Queued);
        assert!(current.work.failure.is_none());
    }
    assert_eq!(
        record(one.reserve_command(reservation).await.unwrap()).receipt,
        admitted.receipt
    );
    assert_eq!(one.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unclaimed_refusal_cannot_overwrite_live_or_bound_work_even_with_current_revision() {
    let (client, name) = database("command_refusal_fences").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let id = &admitted.receipt.envelope.command_id;
    let now = layer2_core::now_rfc3339();
    let claimed = store
        .claim_next_command(claim_request(&agent, "worker", &now))
        .await
        .unwrap()
        .unwrap();
    for revision in [admitted.revision, claimed.revision] {
        assert_eq!(
            store
                .reject_unclaimed_command(
                    id,
                    revision,
                    CommandFailureCode::AuthorityRevoked,
                    now.clone()
                )
                .await,
            Err(CommandLedgerError::StaleClaim)
        );
    }
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    store
        .commit_agent_conversation(message_mutation(&agent, &parent, claim))
        .await
        .unwrap();
    store
        .release_command_claim(claim, now.clone())
        .await
        .unwrap();
    let bound = store.get_command(id).await.unwrap().unwrap();
    assert!(bound.work.lease.is_none());
    assert_eq!(
        store
            .reject_unclaimed_command(
                id,
                bound.revision,
                CommandFailureCode::AuthorityRevoked,
                now
            )
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    assert_eq!(store.get_command(id).await.unwrap(), Some(bound));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn expired_unclaimed_work_is_retained_and_does_not_block_the_next_lane_claim() {
    let (client, name) = database("command_refusal_expired").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let reservation = request(&agent, &parent, "expired");
    let original = record(store.reserve_command(reservation.clone()).await.unwrap());
    let claimed = store
        .claim_next_command(claim_request(&agent, "old", NOW))
        .await
        .unwrap()
        .unwrap();
    let id = &original.receipt.envelope.command_id;
    assert_eq!(
        store
            .reject_unclaimed_command(
                id,
                claimed.revision,
                CommandFailureCode::QueueTimeout,
                NOW.into()
            )
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    store
        .reject_unclaimed_command(
            id,
            claimed.revision,
            CommandFailureCode::QueueTimeout,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let terminal = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(terminal.receipt, original.receipt);
    assert_eq!(
        terminal.work.failure.as_ref().unwrap().code,
        CommandFailureCode::QueueTimeout
    );
    assert_eq!(
        record(store.reserve_command(reservation).await.unwrap()),
        terminal
    );
    let next = record(
        store
            .reserve_command(live_request(&agent, &parent, "next"))
            .await
            .unwrap(),
    );
    let claimed_next = store
        .claim_next_command(claim_request(&agent, "next", &layer2_core::now_rfc3339()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed_next.receipt, next.receipt);
    client.database(&name).drop().await.unwrap();
}
