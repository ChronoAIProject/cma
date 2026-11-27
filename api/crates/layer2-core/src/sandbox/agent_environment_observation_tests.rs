use super::*;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};
#[path = "agent_environment_test_runtime.rs"]
mod runtime_probe;
use runtime_probe::Probe;

async fn observed(
    enabled: bool,
) -> (
    Arc<AgentFileService>,
    Arc<store::InMemorySandboxStore>,
    Arc<Probe>,
    CallerContext,
) {
    let store = Arc::new(store::InMemorySandboxStore::default());
    let mut root = sandbox();
    root.runtime = Some(RuntimeBinding {
        boot_id: Some("b".repeat(32)),
        runtime_id: "runtime-test".into(),
        runtime_version: "image".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    root.lifecycle.home = Some(Box::new(super::super::attachments::persistent_home()));
    root.lifecycle.runtime_generation_high_water = 1;
    store.create_sandbox(root).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    store.create_agent(agent()).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(NoOrganizations),
    ));
    let probe = Arc::new(Probe::default());
    probe.delay_ms.store(1000, Ordering::SeqCst);
    (
        AgentFileService::new(access, probe.clone())
            .with_background_environment(enabled)
            .with_task_experience(Default::default())
            .with_read_timings(true),
        store,
        probe,
        context("owner"),
    )
}
async fn read(
    files: &Arc<AgentFileService>,
    owner: &CallerContext,
    id: &AgentId,
) -> serde_json::Value {
    serde_json::to_value(
        files
            .environment(&owner.caller, Some(owner), None, id)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn environment_pending_is_immediate_and_same_key_observers_share_one_probe() {
    let (files, _, probe, owner) = observed(true).await;
    let start = tokio::time::Instant::now();
    for _ in 0..200 {
        let value = read(&files, &owner, &agent().id).await;
        assert_eq!(value["files"]["state"], "available");
        assert_eq!(value["browser"]["reason"], "observation_pending");
        assert_eq!(value["control"]["writable"], false);
        assert!(value["revalidate_at"].as_str().is_some());
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    let ready = read(&files, &owner, &agent().id).await;
    assert_eq!(ready["browser"]["state"], "not_enabled");
    assert_eq!(ready["control"]["state"], "not_enabled");
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    let outsider = context("other");
    assert!(files
        .environment(&outsider.caller, Some(&outsider), None, &agent().id)
        .await
        .is_err());
    eprintln!("environment fixture: 200 same-key cold requests return pending before 1000ms dependency; probes=1; current authority checked per request");
}

#[tokio::test(start_paused = true)]
async fn disabled_environment_preserves_waited_observation() {
    let (files, _, probe, owner) = observed(false).await;
    let start = tokio::time::Instant::now();
    let value = read(&files, &owner, &agent().id).await;
    assert_eq!(value["browser"]["state"], "not_enabled");
    assert!(start.elapsed() >= Duration::from_secs(1));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn enabled_browser_remains_available_when_the_control_probe_times_out() {
    let (files, _, probe, owner) = observed(true).await;
    let id = agent().id;
    probe.enabled.store(true, Ordering::SeqCst);
    probe.control_delay_ms.store(1_500, Ordering::SeqCst);

    assert_eq!(
        read(&files, &owner, &id).await["browser"]["reason"],
        "observation_pending"
    );
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1_100)).await;
    tokio::task::yield_now().await;

    let pending = read(&files, &owner, &id).await;
    assert_eq!(pending["browser"]["state"], "available");
    assert_eq!(pending["control"]["writable"], false);
    assert!(pending["revalidate_at"].as_str().is_some());

    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    let retry = read(&files, &owner, &id).await;
    assert_eq!(retry["browser"]["state"], "available");
    assert_eq!(retry["control"]["state"], "temporarily_unavailable");
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1_600)).await;
    tokio::task::yield_now().await;

    let settled = read(&files, &owner, &id).await;
    assert_eq!(settled["browser"]["state"], "available");
    assert_eq!(settled["control"]["state"], "unsupported");
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn environment_background_capacity_and_timeout_release_are_bounded() {
    let (files, store, probe, owner) = observed(true).await;
    probe.delay_ms.store(30_000, Ordering::SeqCst);
    for index in 0..200 {
        let mut next = agent();
        next.id = AgentId(format!("agent-{index}"));
        let mut tree = worktree();
        tree.id = crate::WorktreeId(format!("tree-{index}"));
        tree.name = format!("wt-{}", index + 2);
        tree.path = format!("/home/cma/{}/.worktrees/{}", workspace().name, tree.name);
        tree.branch = format!("cma/{}", tree.name);
        tree.agent_id = Some(next.id.clone());
        next.worktree_id = tree.id.clone();
        store.create_worktree(tree).await.unwrap();
        store.create_agent(next.clone()).await.unwrap();
        let value = read(&files, &owner, &next.id).await;
        assert_eq!(value["browser"]["reason"], "observation_pending");
    }
    tokio::task::yield_now().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 16);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 16);
    tokio::time::advance(Duration::from_secs(2)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 16);
    probe.delay_ms.store(10, Ordering::SeqCst);
    read(&files, &owner, &AgentId("agent-199".into())).await;
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 17);
}

#[tokio::test(start_paused = true)]
async fn changed_runtime_cannot_publish_or_reuse_the_old_observation() {
    let (files, store, probe, owner) = observed(true).await;
    probe.enabled.store(true, Ordering::SeqCst);
    read(&files, &owner, &agent().id).await;
    tokio::task::yield_now().await;
    let mut root = store.get_sandbox(&sandbox().id).await.unwrap().unwrap();
    let revision = root.revision;
    root.runtime.as_mut().unwrap().boot_id = Some("c".repeat(32));
    root.runtime.as_mut().unwrap().runtime_id = "runtime-replacement".into();
    root.runtime.as_mut().unwrap().provision_generation = Some(2);
    root.lifecycle.runtime_generation_high_water = 2;
    root.revision += 1;
    store.save_sandbox(revision, root).await.unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    let fresh = read(&files, &owner, &agent().id).await;
    assert_eq!(fresh["browser"]["reason"], "observation_pending");
    assert_eq!(fresh["control"]["writable"], false);
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn pending_and_cached_observations_remain_scoped_to_current_shares() {
    let (files, store, probe, owner) = observed(true).await;
    let access = ResourceAccessService::new(store, Arc::new(NoOrganizations));
    let viewer = context("viewer");
    let token = "a".repeat(64);
    let id = agent().id;
    let share = access
        .create_agent_share_link(
            &owner,
            None,
            CreateAgentShareLink {
                agent_id: id.clone(),
                audience: AgentShareAudience::Public,
                mode: ShareMode::View,
                token: token.clone(),
                idempotency_key: "observation-share".into(),
            },
        )
        .await
        .unwrap();
    access
        .redeem_agent_share_link(&viewer, None, &share.share.id, &token)
        .await
        .unwrap();
    let pending = read(&files, &viewer, &id).await;
    assert_eq!(pending["browser"]["reason"], "observation_pending");
    assert_eq!(pending["control"]["state"], "forbidden");
    read(&files, &owner, &id).await;
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    let current = access
        .read_share(&owner, None, &share.share.selector, &share.share.id)
        .await
        .unwrap();
    access
        .change_share(
            &owner,
            None,
            ShareCommand {
                selector: current.selector,
                change: ShareChange::Revoke {
                    share_id: current.id,
                },
                expected_share_revision: Some(current.revision),
                idempotency_key: "observation-revoke".into(),
            },
        )
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert!(files
        .environment(&viewer.caller, Some(&viewer), None, &id)
        .await
        .is_err());
    assert_eq!(
        read(&files, &owner, &id).await["control"]["writable"],
        false
    );
}

#[tokio::test(start_paused = true)]
async fn request_bound_expiry_cancels_observer_without_renewing_authority() {
    let (files, _, probe, mut owner) = observed(true).await;
    owner.credential_source = CredentialSource::RequestBoundRead {
        expires_at: SystemTime::now() + Duration::from_millis(50),
    };
    let value = read(&files, &owner, &agent().id).await;
    assert_eq!(value["control"]["state"], "forbidden");
    assert_eq!(value["files"]["state"], "forbidden");
    tokio::task::yield_now().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_millis(60)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    // A new independent owner request cannot adopt the expired observation.
    let fresh = read(&files, &context("owner"), &agent().id).await;
    assert_eq!(fresh["browser"]["reason"], "observation_pending");
    tokio::task::yield_now().await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
}
