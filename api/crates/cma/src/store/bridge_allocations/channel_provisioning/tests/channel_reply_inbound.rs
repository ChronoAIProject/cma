use super::*;

pub(super) async fn update(f: &mut Fixture) {
    let id = &f.record.pin.bridge_id;
    let bridge = f.bridges.bridge(id).await.unwrap().unwrap();
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                id,
                bridge.revision,
                "reply-profile-disable",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
    let bridge = f.bridges.bridge(id).await.unwrap().unwrap();
    let mut config = bridge.current().unwrap().config.clone();
    config.display_name = "New inbound profile on retained original context".into();
    let BridgePlacementPolicy::NewSandbox { agent_profile, .. } = &mut config.placement else {
        unreachable!()
    };
    agent_profile.revision += 1;
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                id,
                bridge.revision,
                "reply-profile-update",
                BridgeChange::Update {
                    config: Box::new(config),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
    let bridge = f.bridges.bridge(id).await.unwrap().unwrap();
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                id,
                bridge.revision,
                "reply-profile-enable",
                BridgeChange::Enable,
            ),
        )
        .await
        .unwrap();
}
