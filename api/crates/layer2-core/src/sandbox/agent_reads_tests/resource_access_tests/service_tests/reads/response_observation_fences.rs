use super::*;
use crate::sandbox::store::SandboxWorkerStore;

#[tokio::test]
async fn combined_fallback_release_fences_revocation_and_private_errors() {
    for full in [false, true] {
        for failure in [false, true] {
            let mut f = counted(
                if full {
                    "fallback"
                } else if failure {
                    "unavailable"
                } else {
                    "ready"
                },
                true,
            )
            .await;
            let entered = Arc::new(tokio::sync::Notify::new());
            let resume = Arc::new(tokio::sync::Notify::new());
            if full {
                f.configure_log(|logs| {
                    logs.gate = Some((entered.clone(), resume.clone()));
                    logs.fail = failure;
                });
            } else {
                f.configure_source(|source| source.gate = Some((entered.clone(), resume.clone())));
            }
            let target = ResourceSelector::Agent {
                agent_id: f.agent.id.clone(),
            };
            let share = f
                .store
                .resource_access_snapshot(&target.target())
                .await
                .unwrap()
                .shares[0]
                .clone();
            let authority_calls = f.authority.calls.load(Ordering::SeqCst);
            let (result, ()) = tokio::join!(f.observe(), async {
                entered.notified().await;
                let access = ResourceAccessService::new(f.store.clone(), f.authority.clone());
                access
                    .change_share(&context("owner"), None, revoke(&target, &share))
                    .await
                    .unwrap();
                resume.notify_one();
            });
            assert!(
                matches!(
                    result,
                    Err(AgentReadError::Access(ResourceAccessError::Store(
                        ResourceAccessStoreError::Conflict
                    )))
                ),
                "authority refusal must override source success/error"
            );
            assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
            assert_eq!(f.authority.calls.load(Ordering::SeqCst), authority_calls);
        }
    }
}

#[tokio::test]
async fn combined_complete_fallback_cannot_release_after_deletion() {
    for failure in [false, true] {
        let mut f = counted("fallback", false).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        f.configure_log(|logs| {
            logs.gate = Some((entered.clone(), resume.clone()));
            logs.fail = failure;
        });
        let (result, ()) = tokio::join!(f.observe(), async {
            entered.notified().await;
            let mut sandbox = f
                .store
                .get_sandbox(&f.agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            let revision = sandbox.revision;
            sandbox.status = SandboxStatus::Retired;
            sandbox.retired_at = Some(NOW.into());
            sandbox.purge_after = Some(NOW.into());
            sandbox.revision += 1;
            f.store.save_sandbox(revision, sandbox).await.unwrap();
            f.store
                .purge_sandbox(&f.agent.sandbox_id, revision + 1, NOW)
                .await
                .unwrap();
            resume.notify_one();
        });
        assert!(matches!(result, Err(AgentReadError::NotFound)));
        assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn combined_source_stages_and_release_share_one_io_deadline() {
    for stage in [
        "indexed-window",
        "window-full",
        "full-release",
        "error-release",
    ] {
        let mut f = counted(
            if stage == "error-release" {
                "unavailable"
            } else {
                "fallback"
            },
            false,
        )
        .await;
        match stage {
            "indexed-window" => f.configure_source(|s| {
                s.indexed_delay = Duration::from_secs(8);
                s.window_delay = Duration::from_secs(8);
            }),
            "window-full" => {
                f.configure_source(|s| s.window_delay = Duration::from_secs(8));
                f.configure_log(|s| s.delay = Duration::from_secs(8));
            }
            _ => {
                if stage == "full-release" {
                    f.configure_log(|s| s.delay = Duration::from_secs(8));
                } else {
                    f.configure_source(|s| s.window_delay = Duration::from_secs(8));
                }
                *f.access.delays.lock().unwrap() = vec![Duration::ZERO, Duration::from_secs(8)];
            }
        }
        let start = tokio::time::Instant::now();
        assert!(
            matches!(f.observe().await, Err(AgentReadError::Unavailable(message)) if message == "event read timed out"),
            "{stage}"
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_secs(15) && elapsed < Duration::from_secs(16),
            "{stage}: {elapsed:?}"
        );
        assert_eq!(
            f.access.calls.load(Ordering::SeqCst),
            if stage.ends_with("release") { 2 } else { 1 },
            "{stage}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn combined_observation_never_renews_the_original_shared_grant() {
    let mut f = counted("fallback", true).await;
    let calls = f.authority.calls.load(Ordering::SeqCst);
    let deadline = f.held.revalidation_deadline().unwrap();
    tokio::time::advance(deadline - tokio::time::Instant::now() - Duration::from_secs(1)).await;
    f.configure_source(|s| s.indexed_delay = Duration::from_secs(2));
    assert!(matches!(
        f.observe().await,
        Err(AgentReadError::Access(ResourceAccessError::Access(
            AccessError::RevalidationRequired
        )))
    ));
    assert_eq!(f.authority.calls.load(Ordering::SeqCst), calls);
    assert_eq!(f.source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.logs.calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.held.revalidation_deadline(), Some(deadline));
}

#[tokio::test]
async fn combined_fallback_keeps_the_captured_head_when_the_agent_advances() {
    for mode in ["ready", "fallback"] {
        let mut f = counted(mode, false).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        f.configure_source(|s| s.gate = Some((entered.clone(), resume.clone())));
        let (result, ()) = tokio::join!(f.observe(), async {
            entered.notified().await;
            f.store
                .append_agent_log(
                    &f.agent.id,
                    NOW.into(),
                    AgentLogKind::AgentMessage {
                        content: "later output".into(),
                    },
                )
                .await
                .unwrap();
            let mut agent = f.agent.clone();
            agent.revision += 1;
            agent.conversation.last_seq += 1;
            f.store.save_agent(f.agent.revision, agent).await.unwrap();
            resume.notify_one();
        });
        let AgentResponseObservation::Legacy(snapshot) = result.unwrap() else {
            panic!("legacy")
        };
        assert_eq!(snapshot.head, f.agent.conversation.last_seq);
        assert!(!serde_json::to_string(&snapshot.history)
            .unwrap()
            .contains("later output"));
        assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn combined_fallback_refuses_replaced_agent_incarnations() {
    let mut f = counted("fallback", false).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    f.configure_log(|s| s.gate = Some((entered.clone(), resume.clone())));
    let (result, ()) = tokio::join!(f.observe(), async {
        entered.notified().await;
        let mut agent = f.agent.clone();
        agent.created_at = "2026-09-07T00:00:00Z".into();
        agent.revision += 1;
        f.store.save_agent(f.agent.revision, agent).await.unwrap();
        resume.notify_one();
    });
    assert!(matches!(
        result,
        Err(AgentReadError::Access(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        )))
    ));
    assert_eq!(f.access.calls.load(Ordering::SeqCst), 2);
}
