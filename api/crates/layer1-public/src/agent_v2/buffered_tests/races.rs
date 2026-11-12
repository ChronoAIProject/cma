use super::*;
use serde_json::Value;

#[tokio::test(start_paused = true)]
async fn every_buffered_result_and_dependency_error_is_fenced_after_authority_changes() {
    for route in [SNAPSHOT, HEAD] {
        for fault in ["revoke", "delete", "owner", "expiry"] {
            for fail in [false, true] {
                let f = fixture(true, false).await;
                let shared = matches!(fault, "revoke" | "expiry");
                let share = if shared {
                    Some(super::super::head_tests::share(&f).await)
                } else {
                    None
                };
                let (state, gate, resume) = gated(&f, fail);
                let app = router(state);
                let subject = if shared { "reader" } else { "owner-a" };
                let pending =
                    tokio::spawn(
                        async move { raw(&app, route, subject, "cma-message-bindings").await },
                    );
                entered(&gate).await;
                match fault {
                    "revoke" => revoke(&f, share.unwrap()).await,
                    "expiry" => tokio::time::advance(Duration::from_secs(60)).await,
                    _ => {
                        let mut agent = f
                            .store
                            .get_agent(&AgentId("agt_fixture".into()))
                            .await
                            .unwrap()
                            .unwrap();
                        let revision = agent.revision;
                        if fault == "delete" {
                            agent.status = AgentStatus::Deleted;
                            agent.deleted_at = Some(layer2_core::now_rfc3339());
                        } else {
                            agent.owner = SubjectId::new("changed-owner");
                        }
                        agent.revision += 1;
                        f.store.save_agent(revision, agent).await.unwrap();
                    }
                }
                resume.notify_one();
                let (status, bytes) = pending.await.unwrap();
                assert!(!status.is_success(), "{route} {fault} fail={fail}");
                if fault == "revoke" {
                    // An in-flight share revision change is the existing
                    // Conflict refusal; a fresh authorization sees NotFound.
                    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                    assert_eq!(
                        raw(&router(f.state.clone()), route, "reader", "").await.0,
                        StatusCode::NOT_FOUND
                    );
                }
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                assert!(body.get("items").is_none());
                assert!(body.get("active_turn").is_none());
                let text = String::from_utf8(bytes).unwrap();
                for private in [
                    "Private source input",
                    "private-source-reference",
                    "changed-owner",
                    "cmd_control_source",
                ] {
                    assert!(!text.contains(private), "{fault} leaked {private}");
                }
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_dependency_still_observes_revocation_before_releasing_an_error() {
    for route in [SNAPSHOT, HEAD] {
        let f = fixture(true, false).await;
        let share = super::super::head_tests::share(&f).await;
        let (state, gate, _resume) = gated(&f, false);
        let app = router(state);
        let pending = tokio::spawn(async move { raw(&app, route, "reader", "").await });
        entered(&gate).await;
        revoke(&f, share).await;
        tokio::time::advance(Duration::from_secs(15)).await;
        let (status, bytes) = pending.await.unwrap();
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "agent_read_unavailable");
        assert!(!String::from_utf8_lossy(&bytes).contains("Private source input"));
    }
}

#[tokio::test(start_paused = true)]
async fn revoked_share_fence_overrides_private_failure_and_timeout() {
    for fail in [false, true] {
        let f = fixture(true, false).await;
        let share = super::super::head_tests::share(&f).await;
        let held = f
            .state
            .reads
            .authorize_event_stream(
                &context("reader"),
                Some("fixture-credential"),
                &AgentId("agt_fixture".into()),
            )
            .await
            .unwrap();
        let (state, gate, resume) = gated(&f, fail);
        let pending = tokio::spawn(async move {
            state
                .reads
                .buffered_event_window(&held, &state.commands, |_| {
                    Ok::<_, AgentReadError>(AgentBufferedEventRequest::Head)
                })
                .await
        });
        entered(&gate).await;
        revoke(&f, share).await;
        if fail {
            resume.notify_one();
        } else {
            tokio::time::advance(Duration::from_secs(15)).await;
        }
        assert!(matches!(
            pending.await.unwrap(),
            Err(AgentReadError::Access(ResourceAccessError::Store(
                ResourceAccessStoreError::Conflict
            )))
        ));
    }
}

#[tokio::test]
async fn append_during_binding_preserves_the_captured_page_and_head() {
    for route in [SNAPSHOT, HEAD] {
        let f = fixture(true, false).await;
        let before = f
            .store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap()
            .unwrap();
        let (state, gate, resume) = gated(&f, false);
        let app = router(state);
        let pending = tokio::spawn(async move { raw(&app, route, "owner-a", "").await });
        entered(&gate).await;
        append(
            &f.store,
            AgentLogKind::TitleSet {
                title: "later title".into(),
                source: "owner".into(),
            },
        )
        .await;
        resume.notify_one();
        let (status, bytes) = pending.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["through_cursor"],
            cursor::event(
                body["incarnation"].as_str().unwrap(),
                before.conversation.last_seq
            )
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("later title"));
    }
}
