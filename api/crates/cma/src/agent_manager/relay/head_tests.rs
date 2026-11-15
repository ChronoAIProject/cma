use super::*;
use layer2_core::sandbox::AgentUpdateRecorder;

#[tokio::test]
async fn durable_head_hints_share_live_relays_and_follow_only_current_owners() {
    for count in [2, 3] {
        let (store, instances) = cluster(count).await;
        let id = AgentId("agt-head-hint".into());
        let (owner, authority) = acquire(&store, &instances, &id).await;
        let mut heads: Vec<_> = instances
            .iter()
            .map(|instance| instance.updates.subscribe_head(&id).unwrap())
            .collect();
        // Public v2 streams subscribe to both lanes. Hints use those existing
        // relay connections instead of adding a second stream per reader.
        let _live: Vec<_> = instances
            .iter()
            .map(|instance| instance.updates.subscribe_live(&id).unwrap())
            .collect();
        subscribed(&instances[owner], &id, count).await;
        assert!(instances[owner].updates.notify_owned_head(&id, &authority));
        for head in &mut heads {
            tokio::time::timeout(Duration::from_secs(3), head.recv())
                .await
                .unwrap()
                .unwrap();
        }
        instances[owner].coordinator.begin_drain().await;
        instances[owner]
            .coordinator
            .handoff(&authority, async { Ok(()) })
            .await
            .unwrap();
        assert!(!instances[owner].updates.notify_owned_head(&id, &authority));
        let (successor, next_authority) = acquire(&store, &instances, &id).await;
        subscribed(&instances[successor], &id, count - 1).await;
        assert!(instances[successor]
            .updates
            .notify_owned_head(&id, &next_authority));
        for (index, head) in heads.iter_mut().enumerate() {
            if index != owner {
                tokio::time::timeout(Duration::from_secs(3), head.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
    }
}
