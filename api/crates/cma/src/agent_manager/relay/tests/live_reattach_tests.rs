use super::*;
use layer2_core::sandbox::{AgentLiveEvent, AgentUpdateRecorder};

#[tokio::test]
async fn owner_and_peer_keep_their_subscriptions_across_clean_idle_retirement() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-idle-retirement".into());
    let (owner, first) = acquire(&store, &instances, &id).await;
    let mut lanes = Vec::new();
    for instance in &instances {
        let lane = instance.updates.admit_live(&id).await.unwrap().unwrap();
        assert_eq!(lane.attached_owner(), Some(first.token.clone()));
        lanes.push(lane);
    }
    subscribed(&instances[owner], &id, lanes.len()).await;

    instances[owner]
        .coordinator
        .handoff(&first, async { Ok(()) })
        .await
        .unwrap();
    let (next_owner, successor) = acquire(&store, &instances, &id).await;
    assert!(successor.token.epoch > first.token.epoch);
    for lane in &mut lanes {
        let Ok(AgentLiveEvent::Interrupted(interruption)) =
            tokio::time::timeout(Duration::from_secs(3), lane.recv_event())
                .await
                .unwrap()
        else {
            panic!("the drained lane must report retirement");
        };
        assert_eq!(interruption.owner, first.token);
        let replacement =
            tokio::time::timeout_at(interruption.deadline, lane.successor(&interruption))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(replacement.owner(), &successor.token);
        assert!(replacement.confirm().await);
    }
    subscribed(&instances[next_owner], &id, lanes.len()).await;
    let marker = fragment(&id, "successor data");
    successor
        .run(async {
            instances[next_owner]
                .updates
                .publish_live_fixture(marker.clone());
        })
        .await
        .unwrap();
    for lane in &mut lanes {
        assert_eq!(next(lane).await, marker);
    }
}
