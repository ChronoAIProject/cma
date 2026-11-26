use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn direct_durable_admission_matrix() {
    for deleting in [false, true] {
        let f = Fixture::new(deleting).await;
        for ingress in [f.owner, 1 - f.owner] {
            f.request("healthy", ingress, 202, None).await;
        }
        f.complete().await;
        f.close().await;

        let f = Fixture::new(deleting).await;
        TransportAuthority::revoke(f.authority.as_ref());
        let revision = f.h.stored(&f.agent.id).await.revision;
        for ingress in [f.owner, 1 - f.owner] {
            f.request("receiver_cancelled_live_lease", ingress, 202, None)
                .await;
        }
        assert_eq!(f.h.stored(&f.agent.id).await.revision, revision + 1);
        f.assert_pending_while_cancelled("receiver_cancelled_live_lease")
            .await;
        assert!(f.replicas[f.owner]
            .coordinator
            .acquire(&f.agent.id)
            .await
            .unwrap()
            .is_none());
        f.close().await;

        let f = Fixture::new(deleting).await;
        f.authority.revoke();
        for ingress in [f.owner, 1 - f.owner] {
            f.request("ordinary_stale_authority", ingress, 202, None)
                .await;
        }
        f.assert_pending_while_cancelled("ordinary_stale_authority")
            .await;
        f.close().await;

        let f = Fixture::new(deleting).await;
        let (receiver, ack, server) = super::receiver(&f.authority).await;
        TransportAuthority::revoke(f.authority.as_ref());
        for round in 0..3 {
            assert!(f.replicas[f.owner]
                .coordinator
                .handoff(&f.authority, f.authority.revoke_transports())
                .await
                .is_err());
            assert!(f.ownership.owner(&f.agent.id).await.unwrap().is_some());
            for ingress in [f.owner, 1 - f.owner] {
                f.request(&format!("receiver404_round{round}"), ingress, 202, None)
                    .await;
            }
        }
        f.assert_pending_while_cancelled("receiver404_rounds").await;
        ack.store(true, Ordering::SeqCst);
        f.replicas[f.owner]
            .coordinator
            .handoff(&f.authority, f.authority.revoke_transports())
            .await
            .unwrap();
        assert!(f.ownership.owner(&f.agent.id).await.unwrap().is_none());
        for ingress in [1 - f.owner, f.owner] {
            f.request("exact_ack_recovery", ingress, 202, None).await;
        }
        f.acquire_successor_for_worker().await;
        let successor = f.ownership.owner(&f.agent.id).await.unwrap().unwrap();
        assert!(successor.token.epoch > f.authority.token.epoch);
        f.ownership.release(&f.authority.token).await.unwrap();
        assert_eq!(
            f.ownership.owner(&f.agent.id).await.unwrap().unwrap().token,
            successor.token
        );
        println!(
            "AM503_RECOVERY {}",
            json!({"mode":"ack","revocations":receiver.calls.load(Ordering::SeqCst),"successor_epoch":successor.token.epoch,"old_release_preserved_successor":true})
        );
        f.complete().await;
        server.abort();
        f.close().await;

        let f = Fixture::new(deleting).await;
        let (receiver, _, server) = super::receiver(&f.authority).await;
        TransportAuthority::revoke(f.authority.as_ref());
        let start = tokio::time::Instant::now();
        while f.ownership.owner(&f.agent.id).await.unwrap().is_some() {
            assert!(start.elapsed() < Duration::from_secs(22));
            for replica in &f.replicas {
                replica.coordinator.maintain().await.unwrap();
            }
            assert!(f.replicas[f.owner]
                .coordinator
                .handoff(&f.authority, f.authority.revoke_transports())
                .await
                .is_err());
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        for ingress in [1 - f.owner, f.owner] {
            f.request("real_expiry_receiver_still404", ingress, 202, None)
                .await;
        }
        f.acquire_successor_for_worker().await;
        let successor = f.ownership.owner(&f.agent.id).await.unwrap().unwrap();
        assert!(successor.token.epoch > f.authority.token.epoch);
        assert!(f.replicas[f.owner]
            .coordinator
            .forget_cancelled(&f.authority)
            .await
            .unwrap());
        f.ownership.release(&f.authority.token).await.unwrap();
        assert_eq!(
            f.ownership.owner(&f.agent.id).await.unwrap().unwrap().token,
            successor.token
        );
        println!(
            "AM503_RECOVERY {}",
            json!({"mode":"natural_expiry","wait_ms":start.elapsed().as_millis(),"revocations":receiver.calls.load(Ordering::SeqCst),"successor_epoch":successor.token.epoch,"old_release_preserved_successor":true})
        );
        f.complete().await;
        server.abort();
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn durable_control_store_faults() {
    for deleting in [false, true] {
        for peer in [false, true] {
            let f = Fixture::new(deleting).await;
            let ingress = if peer { 1 - f.owner } else { f.owner };
            // Ownership and idempotency use another Mongo client; fail only control writes.
            f.fail_commands(doc!{"failCommands":["update"],"errorCode":112,"errorLabels":["TransientTransactionError"]},"alwaysOn".into()).await;
            f.request(
                "control_transient_retry_exhaustion",
                ingress,
                503,
                Some("instance_service_unavailable"),
            )
            .await;
            f.disable_failpoint().await;
            assert_eq!(f.failed_commands.load(Ordering::SeqCst), 8);
            assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision);
            f.request("control_store_recovered", ingress, 202, None)
                .await;
            f.complete().await;
            f.close().await;

            let f = Fixture::new(deleting).await;
            f.fail_commands(doc!{"failCommands":["commitTransaction"],"writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"local injected ambiguous commit"}},doc!{"times":5}.into()).await;
            f.request(
                "committed_unknown_reply_reconciled",
                if peer { 1 - f.owner } else { f.owner },
                202,
                None,
            )
            .await;
            f.disable_failpoint().await;
            let revision = f.h.stored(&f.agent.id).await.revision;
            f.request("unknown_reply_replay", f.owner, 202, None).await;
            assert_eq!(f.h.stored(&f.agent.id).await.revision, revision);
            f.complete().await;
            f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn durable_stop_survives_post_commit_cancellation() {
    for peer in [false, true] {
        let f = Fixture::new(false).await;
        f.cancel_after_fence.store(true, Ordering::SeqCst);
        f.request(
            "stop_committed_then_receiver_cancelled",
            if peer { 1 - f.owner } else { f.owner },
            202,
            None,
        )
        .await;
        let after = f.h.stored(&f.agent.id).await;
        assert_ne!(after.work.step, AgentWorkStep::None);
        assert_eq!(after.requested_stop_reason, Some(AgentStopReason::Owner));
        assert!(after.revision > f.agent.revision);
        assert!(f.authority.receiver_cancelled());
        f.assert_pending_while_cancelled("stop_committed_then_receiver_cancelled")
            .await;
        f.cancel_after_fence.store(false, Ordering::SeqCst);
        f.replicas[f.owner]
            .coordinator
            .handoff(&f.authority, f.authority.revoke_transports())
            .await
            .unwrap();
        f.complete().await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn durable_control_background_and_domain_conflict() {
    for deleting in [false, true] {
        let f = Fixture::new(deleting).await;
        // Concurrent real worker on same Agent, plus heartbeat on both replicas.
        let worker = f.replicas[f.owner].worker.clone();
        let coordinator = f.replicas[f.owner].coordinator.clone();
        let id = f.agent.id.clone();
        let task = tokio::spawn(async move {
            for _ in 0..20 {
                let result = worker.run_one(&id, SystemTime::now()).await;
                assert!(
                    result.is_ok()
                        || matches!(
                            result,
                            Err(layer2_core::sandbox::store::SandboxStoreError::Conflict)
                        )
                );
                coordinator.maintain().await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        let result = f
            .request("concurrent_worker_and_heartbeat", 1 - f.owner, 0, None)
            .await;
        assert_eq!(result["status"], 202, "{result}");
        task.await.unwrap();
        if deleting {
            f.request("worker_race_retry_after_quiescence", 1 - f.owner, 202, None)
                .await;
        }
        f.complete().await;
        f.close().await;
    }
    let mut f = Fixture::new(false).await;
    f.deleting = true;
    for ingress in [f.owner, 1 - f.owner] {
        f.request(
            "delete_live_domain_conflict",
            ingress,
            409,
            Some("agent_not_stopped"),
        )
        .await;
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn durable_control_persistent_receiver_failure() {
    for deleting in [false, true] {
        let f = Fixture::new(deleting).await;
        let (first_receiver, _, first_server) = super::receiver(&f.authority).await;
        for ingress in [f.owner, 1 - f.owner] {
            f.request("persistent404_epoch1", ingress, 202, None).await;
        }
        let start = tokio::time::Instant::now();
        while f.ownership.owner(&f.agent.id).await.unwrap().is_some() {
            assert!(start.elapsed() < Duration::from_secs(22));
            for replica in &f.replicas {
                replica.coordinator.maintain().await.unwrap();
            }
            let result = f.replicas[f.owner]
                .coordinator
                .handoff(&f.authority, f.authority.revoke_transports())
                .await;
            assert!(result.is_err());
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // Same acquire path as background worker. A new attempt contacts the
        // still-broken receiver and cancels the newly acquired authority too.
        let next = f.replicas[f.owner]
            .coordinator
            .acquire(&f.agent.id)
            .await
            .unwrap()
            .unwrap();
        assert!(next.token.epoch > f.authority.token.epoch);
        let (next_receiver, next_ack, next_server) = super::receiver(&next).await;
        for ingress in [f.owner, 1 - f.owner] {
            f.request("persistent404_epoch2", ingress, 202, None).await;
        }
        assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision + 1);
        f.assert_pending_while_cancelled("persistent404_epoch2")
            .await;
        println!(
            "AM503_PERSISTENT {}",
            json!({"first_epoch":f.authority.token.epoch,"new_epoch":next.token.epoch,"wait_ms":start.elapsed().as_millis(),"first_receiver_retries":first_receiver.calls.load(Ordering::SeqCst),"both_epochs_cancelled":f.authority.receiver_cancelled() && next.receiver_cancelled()})
        );
        next_ack.store(true, Ordering::SeqCst);
        f.replicas[f.owner]
            .coordinator
            .handoff(&next, next.revoke_transports())
            .await
            .unwrap();
        f.request("persistent404_then_valid_recovery", 1 - f.owner, 202, None)
            .await;
        f.complete().await;
        assert_eq!(next_receiver.calls.load(Ordering::SeqCst), 1);
        first_server.abort();
        next_server.abort();
        f.close().await;
    }
}
