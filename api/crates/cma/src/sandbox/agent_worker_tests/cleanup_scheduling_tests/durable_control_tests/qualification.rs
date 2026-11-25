use super::*;
use layer2_core::qualification::{ControlContext, ControlOperation, ScheduleEffect};
use layer2_core::{OperationClaim, SubjectId};
use sha2::{Digest, Sha256};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn qualified_stop_and_delete_hold_after_durable_scheduling_and_skip_http_replay() {
    for (deleting, peer) in [(false, false), (false, true), (true, false), (true, true)] {
        let registries = std::array::from_fn(|_| Arc::new(cma_qualification::Registry::default()));
        let fixture = Fixture::with_checkpoints(deleting, registries.clone()).await;
        let ingress = if peer {
            1 - fixture.owner
        } else {
            fixture.owner
        };
        let key = uuid::Uuid::new_v4().to_string();
        let (operation, name, body): (_, _, &[u8]) = if deleting {
            (ControlOperation::Delete, "delete", b"")
        } else {
            (ControlOperation::Stop, "stop", b"{}")
        };
        let claim = OperationClaim::new(
            SubjectId::new("owner"),
            format!("agent:{name}:{}", fixture.agent.id),
            &key,
            Sha256::digest(body).into(),
            SystemTime::UNIX_EPOCH,
            Duration::from_secs(8),
        );
        let context = ControlContext::from_claim(operation, &fixture.agent.id, &claim).unwrap();
        let coordinator = &fixture.replicas[ingress].coordinator;
        let capture = coordinator
            .capture_qualification_owner(&fixture.authority.token)
            .await;
        let registry = &registries[ingress];
        let lease = fixture.ownership.owner(&fixture.agent.id).await.unwrap();
        let mut armed = if peer {
            assert!(
                capture.is_none(),
                "peer must refuse capture of the owner authority"
            );
            registry.arm(context, Duration::from_secs(8)).unwrap()
        } else {
            registry
                .arm_with_owner(
                    context,
                    Duration::from_secs(8),
                    capture.expect("owner-local capture"),
                )
                .unwrap()
        };
        let (accepted, durable) = tokio::join!(fixture.raw(ingress, &key, "{}"), async {
            let reached = armed.reached().await.expect("selected checkpoint reached");
            assert_eq!(reached.schedule.effect, ScheduleEffect::Scheduled);
            let durable = fixture.control_receipt().await.expect("durable receipt");
            assert_eq!(
                reached.schedule.agent_revision,
                durable.get_i64("scheduled_revision").unwrap() as u64
            );
            if peer {
                assert!(!armed.cancel_owner(&reached));
                assert!(fixture.authority.is_current());
            } else {
                assert!(armed.cancel_owner(&reached));
                assert!(!armed.cancel_owner(&reached));
                assert!(fixture.authority.receiver_cancelled());
                assert_eq!(
                    fixture.ownership.owner(&fixture.agent.id).await.unwrap(),
                    lease
                );
            }
            armed.release();
            durable
        });
        assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
        let original = accepted.bytes().await.unwrap();
        fixture.completed_response(&key, 202).await;

        let replay_ingress = 1 - ingress;
        let armed = registries[replay_ingress]
            .arm(context, Duration::from_secs(8))
            .unwrap();
        let replay = tokio::time::timeout(
            Duration::from_secs(4),
            fixture.raw(replay_ingress, &key, "{}"),
        )
        .await
        .expect("cached replay must not reach the armed checkpoint");
        drop(armed);
        assert_eq!(replay.status(), reqwest::StatusCode::ACCEPTED);
        assert_eq!(replay.bytes().await.unwrap(), original);
        assert_eq!(fixture.control_receipt().await, Some(durable));

        if !peer {
            fixture
                .assert_pending_while_cancelled("qualification_cancel_owner")
                .await;
            coordinator
                .handoff(&fixture.authority, fixture.authority.revoke_transports())
                .await
                .unwrap();
        }
        fixture.complete().await;
        fixture.close().await;
    }
}
