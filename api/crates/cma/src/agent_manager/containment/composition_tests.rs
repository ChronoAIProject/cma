use super::*;
use layer2_core::agent_ownership::Settlement;
use mongodb::bson::{doc, DateTime, Document};

#[path = "composition_test_support.rs"]
mod support;
use support::{configured, ClassifiedReceiver};

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn all_flag_combinations_dispatch_only_the_selected_housekeeping_consumer() {
    for combined in [false, true] {
        for receiver_only in [false, true] {
            let (client, name, manager, id) = configured(combined, receiver_only).await;
            // Drive the actual housekeeping entry deterministically; activation
            // and the owned periodic supervisor are exercised separately below.
            manager.coordinator.activate().await.unwrap();
            let old = manager.coordinator.acquire(&id).await.unwrap().unwrap();
            let healthy = manager
                .coordinator
                .acquire(&AgentId("healthy".into()))
                .await
                .unwrap()
                .unwrap();
            let receiver =
                ClassifiedReceiver::install(&old, Some(ReceiverSettlementFailure::Unavailable));
            manager.maintain_receivers(vec![old.clone()]).await.unwrap();
            if combined {
                manager.maintain_receivers(vec![]).await.unwrap();
            }
            assert!(!old.is_current());
            assert!(healthy.is_current() && manager.coordinator.is_ready());
            assert_eq!(manager.contained.pending(), usize::from(!combined));
            assert_eq!(
                receiver.typed_calls.load(Ordering::Acquire),
                usize::from(combined)
            );
            assert_eq!(
                receiver.legacy_calls.load(Ordering::Acquire),
                usize::from(!combined)
            );
            assert!(manager
                .coordinator
                .store
                .owner(&id)
                .await
                .unwrap()
                .is_some());
            if combined {
                assert!(manager.handoffs.lock().unwrap().is_empty());
                assert!(manager.retired_since.lock().unwrap().is_empty());
                assert!(!manager
                    .coordinator
                    .forget_unconfirmed_retirement(&old)
                    .await
                    .unwrap());
            }
            *receiver.failure.lock().unwrap() = None;
            manager.maintain_receivers(vec![]).await.unwrap();
            assert!(manager
                .coordinator
                .store
                .owner(&id)
                .await
                .unwrap()
                .is_none());
            assert_eq!(manager.contained.pending(), 0);
            assert_eq!(healthy.run(async { 42 }).await.unwrap(), 42);
            manager.drain().await.unwrap();
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn legacy_supervisor_survives_unavailable_receiver_and_retries_fenced_owner() {
    let (client, name, manager, healthy_id) = configured(false, false).await;
    manager.activate().await.unwrap();
    let supervisor = manager.supervisor.lock().await.as_ref().unwrap().id();
    let healthy = manager
        .coordinator
        .acquire(&healthy_id)
        .await
        .unwrap()
        .unwrap();
    let inactive = AgentId("retired-agent".into());
    let old = manager
        .coordinator
        .acquire(&inactive)
        .await
        .unwrap()
        .unwrap();
    let receiver = ClassifiedReceiver::install(&old, Some(ReceiverSettlementFailure::Unavailable));
    manager
        .retired_since
        .lock()
        .unwrap()
        .insert(inactive.clone(), Instant::now() - Duration::from_secs(61));

    tokio::time::timeout(Duration::from_secs(8), async {
        while manager.contained.pending() == 0 {
            assert!(manager.is_ready());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(receiver.legacy_calls.load(Ordering::Acquire) > 0);
    assert_eq!(receiver.typed_calls.load(Ordering::Acquire), 0);
    assert!(!old.is_current());
    assert_eq!(
        manager
            .coordinator
            .store
            .owner(&inactive)
            .await
            .unwrap()
            .unwrap()
            .token,
        old.token
    );
    assert!(healthy.is_current() && manager.is_ready());
    assert!(!*manager.coordinator.failure_signal().borrow());
    *receiver.failure.lock().unwrap() = None;

    tokio::time::timeout(Duration::from_secs(8), async {
        while manager.contained.pending() != 0 {
            assert!(manager.is_ready());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(receiver.legacy_calls.load(Ordering::Acquire) >= 2);
    assert!(manager
        .coordinator
        .store
        .owner(&inactive)
        .await
        .unwrap()
        .is_none());
    let task = manager.supervisor.lock().await;
    let running = task.as_ref().unwrap();
    assert_eq!(running.id(), supervisor);
    assert!(!running.is_finished());
    drop(task);
    assert_eq!(healthy.run(async { 42 }).await.unwrap(), 42);
    manager.drain().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn receiver_only_flag_cannot_hide_typed_fatal_results_in_combined_dispatch() {
    for receiver_only in [false, true] {
        for failure in [
            ReceiverSettlementFailure::IncompatibleProtocol,
            ReceiverSettlementFailure::AuthorityLost,
            ReceiverSettlementFailure::Invalid,
            ReceiverSettlementFailure::Unclassified,
        ] {
            let (client, name, manager, id) = configured(true, receiver_only).await;
            manager.coordinator.activate().await.unwrap();
            let old = manager.coordinator.acquire(&id).await.unwrap().unwrap();
            let receiver = ClassifiedReceiver::install(&old, Some(failure));
            manager.maintain_receivers(vec![old.clone()]).await.unwrap();
            let error = manager.maintain_receivers(vec![]).await.unwrap_err();
            match failure {
                ReceiverSettlementFailure::IncompatibleProtocol => {
                    assert_eq!(error, OwnershipError::IncompatibleProtocol)
                }
                ReceiverSettlementFailure::AuthorityLost => {
                    assert_eq!(error, OwnershipError::Stale)
                }
                _ => assert!(matches!(error, OwnershipError::Invalid(_))),
            }
            assert_eq!(manager.contained.pending(), 0);
            assert_eq!(receiver.legacy_calls.load(Ordering::Acquire), 0);
            assert_eq!(receiver.typed_calls.load(Ordering::Acquire), 1);
            assert!(!old.is_current());
            assert_eq!(manager.coordinator.owned().await.len(), 1);
            assert!(manager
                .coordinator
                .store
                .owner(&id)
                .await
                .unwrap()
                .is_some());
            manager.stop_admission();
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn combined_flags_retain_absent_floor_through_late_same_token_completion() {
    for receiver_only in [false, true] {
        let (client, name, manager, id) = configured(true, receiver_only).await;
        manager.coordinator.activate().await.unwrap();
        let old = manager.coordinator.acquire(&id).await.unwrap().unwrap();
        let receiver =
            ClassifiedReceiver::install(&old, Some(ReceiverSettlementFailure::Unavailable));
        manager.maintain_receivers(vec![old.clone()]).await.unwrap();
        manager.maintain_receivers(vec![]).await.unwrap();
        let owners = client
            .database(&name)
            .collection::<Document>("cma_agent_manager_owners");
        let filter = doc! {"_id": id.as_str()};
        let row = owners.find_one(filter.clone()).await.unwrap().unwrap();
        let expires = *row.get_datetime("expires_at").unwrap();
        owners
            .update_one(
                filter.clone(),
                doc! {"$set": {"expires_at": DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
        assert!(manager
            .coordinator
            .store
            .owner(&id)
            .await
            .unwrap()
            .is_none());
        manager.maintain_receivers(vec![]).await.unwrap();
        assert!(!manager
            .coordinator
            .forget_unconfirmed_retirement(&old)
            .await
            .unwrap());
        assert_eq!(manager.coordinator.owned().await.len(), 1);
        assert_eq!(manager.contained.pending(), 0);
        // Model durable visibility of a delayed same-token renewal after
        // absence. Core barrier tests also exercise the owned operation task.
        owners
            .update_one(filter.clone(), doc! {"$set": {"expires_at": expires}})
            .await
            .unwrap();
        manager.maintain_receivers(vec![]).await.unwrap();
        assert!(manager.coordinator.acquire(&id).await.unwrap().is_none());
        assert!(!old.is_current());
        assert!(old.run(async { 42 }).await.is_err());
        owners
            .update_one(
                filter,
                doc! {"$set": {"expires_at": DateTime::from_millis(0)}},
            )
            .await
            .unwrap();
        let next = manager.coordinator.acquire(&id).await.unwrap().unwrap();
        assert!(next.is_current() && next.token.epoch > old.token.epoch);
        manager.maintain_receivers(vec![]).await.unwrap();
        assert_eq!(
            manager.containment_times.lock().unwrap().entries[&id].token,
            next.token
        );
        assert_eq!(
            manager
                .coordinator
                .retirement_background(&old)
                .await
                .unwrap(),
            Some(Settlement::Superseded {
                newer_epoch: next.token.epoch
            })
        );
        manager.containment_times.lock().unwrap().remove(&old);
        let target = layer2_core::sandbox::AgentRuntimeTarget {
            agent_id: id.clone(),
            conversation_id: "successor-conversation".into(),
            runtime_generation: 1,
        };
        let (_send, received) = tokio::sync::mpsc::channel(1);
        next.run(async {
            let attachment = manager
                .registry
                .attach(
                    id.clone(),
                    target.conversation_id.clone(),
                    1,
                    1,
                    layer2_core::RuntimeCapabilities::default(),
                    layer3_adapter::ConversationInputSender::spawn(1, |_| async { Ok(()) }),
                    received,
                    Arc::new(support::UnexpectedUpdate),
                )
                .unwrap();
            manager.registry.publish_and_activate(&attachment).unwrap();
        })
        .await
        .unwrap();
        assert!(layer2_core::sandbox::AgentRuntimeRegistry::resolve(
            manager.registry.as_ref(),
            &target
        )
        .await
        .is_ok());
        manager.registry.detach_owner(&old.token);
        assert!(layer2_core::sandbox::AgentRuntimeRegistry::resolve(
            manager.registry.as_ref(),
            &target
        )
        .await
        .is_ok());
        assert_eq!(
            manager.containment_times.lock().unwrap().entries[&id].token,
            next.token
        );
        assert!(Arc::ptr_eq(
            &manager.coordinator.local(&id).await.unwrap(),
            &next
        ));
        assert_eq!(receiver.legacy_calls.load(Ordering::Acquire), 0);
        assert_eq!(manager.contained.pending(), 0);
        assert_eq!(next.run(async { 42 }).await.unwrap(), 42);
        manager.drain().await.unwrap();
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn all_configured_modes_activate_once_and_dormant_shutdown_registers_nothing() {
    for combined in [false, true] {
        for receiver_only in [false, true] {
            for activate in [false, true] {
                let (client, name, manager, _) = configured(combined, receiver_only).await;
                assert!(manager
                    .coordinator
                    .store
                    .membership()
                    .await
                    .unwrap()
                    .members
                    .is_empty());
                if activate {
                    let (first, second) = tokio::join!(manager.activate(), manager.activate());
                    first.unwrap();
                    second.unwrap();
                    let task = manager.supervisor.lock().await.as_ref().unwrap().id();
                    manager.activate().await.unwrap();
                    assert_eq!(manager.supervisor.lock().await.as_ref().unwrap().id(), task);
                    assert_eq!(
                        manager
                            .coordinator
                            .store
                            .membership()
                            .await
                            .unwrap()
                            .members
                            .len(),
                        1
                    );
                    assert!(manager.is_ready());
                }
                manager.drain().await.unwrap();
                assert!(!manager.is_ready());
                assert!(manager.activate().await.is_err());
                if !activate {
                    assert!(manager
                        .coordinator
                        .store
                        .membership()
                        .await
                        .unwrap()
                        .members
                        .is_empty());
                    assert!(manager.supervisor.lock().await.is_none());
                }
                client.database(&name).drop().await.unwrap();
            }
        }
    }
}
