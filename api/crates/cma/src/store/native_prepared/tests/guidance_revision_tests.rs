use super::*;

fn revision_one(mut plan: NativePreparedPlan) -> NativePreparedPlan {
    let workspace = &mut plan.warm.workspaces[0];
    workspace.agents[0].revision = 1;
    workspace.profile.content.agent_profiles[0].revision = 1;
    workspace.profile.digest = content_digest(&workspace.profile.content).unwrap();
    plan.validate().unwrap();
    plan
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_guidance_revision_replaces_only_unclaimed_older_homes() {
    let (client, name, store) = database().await;
    let current = plan();
    assert_eq!(current.warm.workspaces[0].agents[0].revision, 2);
    let old = revision_one(current.clone());
    assert_ne!(old, current);
    store
        .reconcile_native_prepared_target(Some(&old), 1, NOW)
        .await
        .unwrap();
    let original = publish_ready(&store).await;
    let input = admission(&store, &old, "guidance-owner", "guidance-owned").await;
    let SandboxAdmissionResult::Created(owned) = admit_at(&store, input, NOW).await.unwrap() else {
        panic!("fresh owner admission expected");
    };
    assert_eq!(owned.id, original.payload.id);
    let claimed = records(&store)
        .await
        .into_iter()
        .find(|row| row.payload.id == owned.id)
        .unwrap();
    assert_eq!(claimed.payload.state, WarmSandboxState::Claimed);
    store
        .reconcile_native_prepared_target(Some(&old), 1, NOW)
        .await
        .unwrap();
    let unclaimed = publish_ready(&store).await;
    store
        .reconcile_native_prepared_target(Some(&current), 1, NOW)
        .await
        .unwrap();
    let next = records(&store).await;
    assert_eq!(
        next.iter().find(|row| row.payload.id == owned.id).unwrap(),
        &claimed
    );
    assert_eq!(store.get_sandbox(&owned.id).await.unwrap().unwrap(), owned);
    assert_eq!(
        next.iter()
            .find(|row| row.payload.id == unclaimed.payload.id)
            .unwrap()
            .payload
            .state,
        WarmSandboxState::Retiring
    );
    let replacement = next
        .iter()
        .find(|row| row.payload.state == WarmSandboxState::Preparing)
        .unwrap();
    assert_eq!(replacement.plan, current);
    assert_ne!(replacement.payload.id, unclaimed.payload.id);
    assert!(replacement.ownership.is_none());
    client.database(&name).drop().await.unwrap();
}
