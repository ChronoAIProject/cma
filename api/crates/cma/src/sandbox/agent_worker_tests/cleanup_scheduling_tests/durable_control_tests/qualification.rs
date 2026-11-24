use super::*;
use layer2_core::qualification::{ControlContext, ControlOperation, ScheduleEffect};
use layer2_core::{OperationClaim, SubjectId};
use sha2::{Digest, Sha256};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn qualified_stop_and_delete_hold_after_durable_scheduling_and_skip_http_replay() {
    for deleting in [false, true] {
        let registry = Arc::new(cma_qualification::Registry::default());
        let fixture = Fixture::with_checkpoint(deleting, registry.clone()).await;
        let ingress = if deleting {
            fixture.owner
        } else {
            1 - fixture.owner
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
        let mut armed = registry.arm(context, Duration::from_secs(8)).unwrap();
        let (accepted, durable) = tokio::join!(fixture.raw(ingress, &key, "{}"), async {
            let reached = armed.reached().await.expect("selected checkpoint reached");
            assert_eq!(reached.schedule.effect, ScheduleEffect::Scheduled);
            let durable = fixture.control_receipt().await.expect("durable receipt");
            assert_eq!(
                reached.schedule.agent_revision,
                durable.get_i64("scheduled_revision").unwrap() as u64
            );
            armed.release();
            durable
        });
        assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
        let original = accepted.bytes().await.unwrap();
        fixture.completed_response(&key, 202).await;

        let armed = registry.arm(context, Duration::from_secs(8)).unwrap();
        let replay =
            tokio::time::timeout(Duration::from_secs(4), fixture.raw(1 - ingress, &key, "{}"))
                .await
                .expect("cached replay must not reach the armed checkpoint");
        drop(armed);
        assert_eq!(replay.status(), reqwest::StatusCode::ACCEPTED);
        assert_eq!(replay.bytes().await.unwrap(), original);
        assert_eq!(fixture.control_receipt().await, Some(durable));

        fixture.complete().await;
        fixture.close().await;
    }
}
