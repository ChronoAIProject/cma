use super::*;
use layer2_core::{
    profiles::{PermissionLevel, ThinkingEffort},
    sandbox::AgentSettings,
};

#[tokio::test]
async fn peer_reopen_observes_renewal_of_a_short_initial_lease() {
    let (fixture, parent, agent, _) = retained_reopen_tests::restored_reopen().await;
    let (parent, _) = ready_on_wall_clock(&fixture, parent).await;
    let (ownership, replicas) = cluster(2).await;
    let (owner, _) = acquire(&ownership, &replicas, &agent.id).await;
    let _workers = workers_with_renewal(&fixture.harness, &replicas, true);
    let now = SystemTime::now();
    let store = &fixture.harness.store;
    let claimed = store
        .claim_sandbox_work(
            &parent.id,
            "short-reopen-lease",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(3)),
        )
        .await
        .unwrap()
        .unwrap();
    let old_expiry = stamp_time(&claimed.work.lease.as_ref().unwrap().expires_at);
    fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
    let caller = replicas[(owner + 1) % replicas.len()].relay.clone();
    let attempted = claimed.clone();
    let reopen =
        tokio::spawn(async move { caller.reopen_agent(&attempted, &agent.id, false).await });

    tokio::time::timeout(
        Duration::from_secs(2),
        fixture.backend.reopen_entered.notified(),
    )
    .await
    .expect("the signed peer must enter reconnect under the short lease");
    let renewed_at = SystemTime::now();
    let refreshed = store
        .renew_sandbox_work(
            &claimed,
            &rfc3339_at(renewed_at),
            &rfc3339_at(renewed_at + Duration::from_secs(15)),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(stamp_time(&refreshed) > old_expiry);
    tokio::time::sleep(
        old_expiry
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            + Duration::from_millis(100),
    )
    .await;
    fixture.backend.reopen_release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), reopen)
        .await
        .unwrap()
        .unwrap()
        .expect("the same renewed claim must authorize reconnect after its old expiry");
}

#[tokio::test]
async fn issue_1164_peer_delete_fences_reconnect_and_settings_dispatch() {
    for hold_adoption in [true, false] {
        let settings = AgentSettings {
            model: "fixture".into(),
            reasoning_effort: ThinkingEffort::Medium,
            permission_mode: PermissionLevel::Guarded,
            system_prompt: None,
        };
        let (fixture, parent, agent, _) =
            retained_reopen_tests::restored_reopen_with_settings(false, Some(settings)).await;
        let (parent, now) = ready_on_wall_clock(&fixture, parent).await;
        let (ownership, replicas) = cluster(2).await;
        let (owner, _) = acquire(&ownership, &replicas, &agent.id).await;
        let workers = workers_with_renewal(&fixture.harness, &replicas, true);
        let claim = fixture
            .harness
            .store
            .claim_sandbox_work(
                &parent.id,
                "reopen-dispatch",
                rfc3339_at(now),
                rfc3339_at(SystemTime::now() + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
        let pause = Arc::new(super::super::super::backend::RetirementProbePause::default());
        if hold_adoption {
            *fixture.backend.adoption_pause.lock().unwrap() = Some(pause.clone());
        }
        fixture.backend.hold_reopen.store(true, Ordering::SeqCst);
        let inputs = fixture.backend.inputs.lock().unwrap().len();
        let relay = replicas[(owner + 1) % replicas.len()].relay.clone();
        let agent_id = agent.id.clone();
        let running =
            tokio::spawn(async move { relay.reopen_agent(&claim, &agent_id, false).await });
        let (entered, release) = if hold_adoption {
            (&pause.entered, &pause.release)
        } else {
            (
                fixture.backend.reopen_entered.as_ref(),
                fixture.backend.reopen_release.as_ref(),
            )
        };
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("the signed peer must reach the chosen adapter boundary");
        let app = router(&fixture);
        // Complete DELETE and release the previous stage before the periodic
        // observation could see it, so this exercises the dispatch check.
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            let reply = public_call(
                &app,
                &parent.id,
                Method::DELETE,
                "owner",
                StatusCode::ACCEPTED,
            )
            .await;
            assert_eq!(reply["status"], "retiring");
            release.notify_one();
            running.await.unwrap()
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(
            fixture.backend.hold_reopen.load(Ordering::SeqCst),
            hold_adoption,
            "a retired adoption must not reach reconnect"
        );
        assert_eq!(fixture.backend.inputs.lock().unwrap().len(), inputs);
        assert!(!workers[owner].1.target_is_attached(
            &crate::sandbox::live_agents::LiveAgentTarget {
                agent_id: agent.id,
                conversation_id: agent.conversation.conversation_id.unwrap(),
                runtime_generation: parent.runtime.unwrap().provision_generation.unwrap(),
            }
        ));
    }
}
