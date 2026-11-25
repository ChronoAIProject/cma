use std::sync::Arc;

use layer2_core::profiles::content_digest;
use layer2_core::sandbox::{AgentStatus, FirstMessageDelivery};
use layer2_core::trigger_domain::{
    LaunchAuthorityClass, LaunchEffectKind, LaunchResolution, LaunchState, LaunchWorkStep,
    ResolvedResource, TriggerId, TriggerImage, TriggerInlineAgent, TriggerInlineTarget,
    TriggerRevision,
};
use layer2_core::Capability;

use super::fixtures_tests::{
    frozen_inputs, started_launch, test_worker, time, FakeDriver, Harness, TestLaunchStore,
};

#[tokio::test]
async fn scheduled_launch_follows_ready_resources_without_waiting_between_stages() {
    let harness = Harness::pending();
    let now = super::fixtures_tests::time(1);
    assert_eq!(harness.worker.run_due(now).await.unwrap(), 1);
    let pending = harness.launch();
    assert_eq!(pending.state, LaunchState::MessagePending);
    assert_eq!(harness.driver.resource_counts(), (1, 1, 1));
    assert_eq!(harness.driver.profile_queries().len(), 1);
    assert_eq!(harness.driver.trigger_queries().len(), 1);
    // The first prompt is still queued: the scheduler must respect its
    // deferred timestamp and must not spin or repeat resource effects.
    assert_eq!(harness.worker.run_due(now).await.unwrap(), 0);
    assert_eq!(harness.driver.calls().len(), 3);
    harness
        .driver
        .complete_first_message(pending.resolution.agent.id().unwrap(), 2);
    assert_eq!(
        harness
            .worker
            .run_due(super::fixtures_tests::time(2))
            .await
            .unwrap(),
        1
    );
    assert_eq!(harness.launch().state, LaunchState::Ready);
    assert_eq!(harness.driver.calls().len(), 3);
    let event_head = harness.launch().event_head;
    for minute in 3..10 {
        harness
            .worker
            .run_due(super::fixtures_tests::time(minute))
            .await
            .unwrap();
    }
    assert_eq!(
        harness.launch().event_head,
        event_head,
        "unchanged Ready monitoring must not grow Launch history"
    );
}

#[tokio::test]
async fn nothing_existing_reaches_ready_and_completes_only_after_the_agent_stops() {
    let harness = Harness::pending();
    for minute in 1..=6 {
        harness.run(minute).await;
    }

    let pending = harness.launch();
    assert_eq!(pending.state, LaunchState::MessagePending);
    assert_eq!(pending.work.step, LaunchWorkStep::DeliverFirstMessage);
    let agent_id = pending.resolution.agent.id().unwrap().clone();
    let agent = harness.driver.agent_value(&agent_id);
    assert_eq!(agent.status, AgentStatus::Ready);
    assert_eq!(harness.driver.resource_counts(), (1, 1, 1));

    let calls = harness.driver.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].kind, LaunchEffectKind::CreateSandbox);
    assert_eq!(calls[1].kind, LaunchEffectKind::CreateWorkspace);
    assert_eq!(calls[2].kind, LaunchEffectKind::CreateAgent);
    assert_eq!(
        calls[2].first_message.as_deref(),
        Some("Inspect the workspace")
    );

    harness.driver.complete_first_message(&agent_id, 7);
    harness.run(7).await;
    let observed = harness.launch();
    assert_eq!(observed.first_message, FirstMessageDelivery::Completed);
    assert_eq!(observed.work.step, LaunchWorkStep::Finish);
    let record = harness
        .driver
        .agent_value(&agent_id)
        .conversation
        .first_message
        .unwrap();
    let evidence = observed.first_message_evidence.as_ref().unwrap();
    assert_eq!(evidence.agent_id, agent_id);
    assert_eq!(evidence.origin_key, record.origin_key);
    assert_eq!(evidence.payload_sha256, record.payload_sha256);

    harness.run(8).await;
    assert_eq!(harness.launch().state, LaunchState::Ready);
    harness.run(9).await;
    assert_eq!(harness.launch().state, LaunchState::Ready);
    assert_eq!(harness.driver.calls().len(), 3);

    harness.driver.stop_agent(&agent_id, 10);
    harness.run(10).await;
    let completed = harness.launch();
    assert_eq!(completed.state, LaunchState::Completed);
    assert_eq!(completed.work.step, LaunchWorkStep::None);
    assert_eq!(harness.driver.calls().len(), 3);
}

#[tokio::test]
async fn worker_reads_the_frozen_trigger_and_profiles_after_republish() {
    let harness = Harness::pending();
    let republished = frozen_inputs(
        harness.inputs.trigger_id.clone(),
        "republished",
        2,
        "A newer message",
        false,
    );
    harness.driver.install_context(republished);

    for minute in 1..=5 {
        harness.run(minute).await;
    }

    let queries = harness.driver.trigger_queries();
    assert!(!queries.is_empty());
    assert!(queries.iter().all(|(trigger_id, digest)| {
        trigger_id == &harness.inputs.trigger_id && digest == &harness.inputs.revision.digest
    }));
    assert!(harness
        .driver
        .profile_queries()
        .iter()
        .all(|target| { target == &harness.inputs.revision.content.target }));

    let calls = harness.driver.calls();
    assert_eq!(calls[0].profile_id.as_deref(), Some("sbp_worker"));
    assert_eq!(calls[1].profile_id.as_deref(), Some("wkp_worker"));
    assert_eq!(calls[2].profile_id.as_deref(), Some("agp_worker"));
    assert_eq!(
        calls[2].first_message.as_deref(),
        Some("Inspect the workspace")
    );
    let agent_id = harness.launch().resolution.agent.id().unwrap().clone();
    assert_eq!(
        harness.driver.agent_value(&agent_id).profile.content,
        harness.inputs.profiles.agent.content
    );
}

#[tokio::test]
async fn retained_launch_provisions_its_pinned_runtime_after_republish() {
    let id = TriggerId("trg_retained_image".into());
    let mut retained = frozen_inputs(id.clone(), "retained", 1, "Start", false);
    let pinned = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    retained.profiles.sandbox.content.runtime_image = Some(pinned.clone());
    retained.profiles.sandbox.digest = content_digest(&retained.profiles.sandbox.content).unwrap();
    let mut content = retained.revision.content.clone();
    let profile = &retained.profiles.agent.content;
    content.target.inline = Some(TriggerInlineTarget {
        image: Some(TriggerImage {
            id: "retained-image".into(),
            version: Some("retained-version".into()),
            reference: Some(pinned.clone()),
        }),
        agent: TriggerInlineAgent {
            model: profile.model.clone(),
            reasoning_effort: profile.thinking_effort,
            permission_mode: profile.permission_level,
            skills: profile.skills.clone(),
            system_prompt: profile.system_prompt.clone(),
            services: Vec::new(),
        },
        workspace: None,
    });
    retained.revision =
        TriggerRevision::new(1, content, retained.revision.published_at.clone()).unwrap();
    let launch = started_launch(
        &retained,
        LaunchAuthorityClass::Visitor,
        LaunchResolution::pending(),
        Vec::new(),
    );
    let launch_id = launch.id.clone();
    let store = Arc::new(TestLaunchStore::default());
    store.insert(launch);
    let driver = FakeDriver::new(retained);
    driver.install_context(frozen_inputs(id, "default", 2, "Start", false));
    let worker = test_worker(store.clone(), driver.clone(), "new-replica");

    assert_eq!(worker.run_due(time(1)).await.unwrap(), 1);
    let stored = store.stored(&launch_id);
    let sandbox = driver.sandbox_value(stored.resolution.sandbox.id().unwrap());
    assert_eq!(sandbox.profile.content.runtime_image, Some(pinned));
    assert_eq!(stored.state, LaunchState::MessagePending);
}

#[tokio::test]
async fn resource_effects_project_only_the_launch_authority_class() {
    for (class, expected) in [
        (LaunchAuthorityClass::Visitor, Capability::LaunchTriggers),
        (
            LaunchAuthorityClass::SandboxCreator,
            Capability::CreateSandbox,
        ),
    ] {
        let harness = Harness::new(class, LaunchResolution::pending(), Vec::new(), false);
        for minute in 1..=5 {
            harness.run(minute).await;
        }
        let calls = harness.driver.calls();
        assert_eq!(calls.len(), 3);
        for call in calls {
            assert_eq!(call.subject.as_str(), "launch-owner");
            assert_eq!(call.capabilities, vec![expected]);
            assert_eq!(call.authority, Some(expected));
            assert_eq!(call.launch_id, harness.launch_id.as_str());
        }
    }
}

#[tokio::test]
async fn newly_created_sandbox_rediscovers_its_profile_installed_workspace() {
    let harness = Harness::new(
        LaunchAuthorityClass::Visitor,
        LaunchResolution::pending(),
        Vec::new(),
        true,
    );
    harness.driver.enable_installed_target_workspace();

    harness.run(1).await;
    harness.run(2).await;
    let discovered = harness.launch();
    assert_eq!(discovered.state, LaunchState::WorkspacePending);
    assert!(matches!(
        discovered.resolution.sandbox,
        ResolvedResource::Created(_)
    ));
    assert!(matches!(
        discovered.resolution.workspace,
        ResolvedResource::Reused(_)
    ));

    harness.run(3).await;
    harness.run(4).await;
    assert_eq!(harness.launch().state, LaunchState::AgentPending);
    assert_eq!(harness.driver.resource_counts(), (1, 1, 0));
    assert_eq!(
        harness
            .driver
            .calls()
            .iter()
            .filter(|call| call.kind == LaunchEffectKind::CreateWorkspace)
            .count(),
        0
    );
}
