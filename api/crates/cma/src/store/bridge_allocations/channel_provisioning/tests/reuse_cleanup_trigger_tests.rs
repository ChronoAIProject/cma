use super::*;

impl CleanupFixture {
    async fn trigger_reuse(&self, key: &str, prompt: bool) -> AgentAdmission {
        let agent = self
            .f
            .f
            .original
            .resources
            .get_agent(&self.original.placement.target().agent_id)
            .await
            .unwrap()
            .unwrap();
        let at = layer2_core::now_rfc3339();
        let origin = AgentOrigin::Trigger {
            trigger_id: "trg_cleanup_reuse".into(),
            launch_id: format!("lch_{key}"),
        };
        let creation_key = format!(
            "trigger-profile:{}",
            layer2_core::profiles::content_digest(&origin).unwrap()
        );
        AgentAdmission {
            created_by: Some(caller().actor),
            owner: agent.owner,
            workspace_id: agent.workspace_id,
            agent_id: layer2_core::AgentId::generate(),
            worktree_id: layer2_core::WorktreeId::generate(),
            profile: agent.profile,
            capabilities: agent.capabilities,
            title: None,
            first_message: prompt.then(|| {
                FirstMessageRecord::new(creation_key.clone(), "New trigger work", at.clone())
                    .unwrap()
            }),
            origin,
            creation_key,
            event_at: at,
        }
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_closure_excludes_new_trigger_reuse_and_preserves_accepted_binding() {
    for prompt in [false, true] {
        let mut f = CleanupFixture::new(true).await;
        f.reset().await;
        let accepted = f.trigger_reuse("before_closure", false).await;
        let result =
            f.f.f
                .original
                .resources
                .admit_agent_reusing_profile(accepted.clone())
                .await
                .unwrap();
        assert_eq!(
            result.creation().agent.id,
            f.original.placement.target().agent_id
        );
        f.request_cleanup().await.unwrap();
        f.close_admission().await.unwrap();
        let before = f
            .row("agents", f.original.placement.target().agent_id.as_str())
            .await;
        let replay =
            f.f.f
                .original
                .resources
                .admit_agent_reusing_profile(accepted)
                .await
                .unwrap();
        assert_eq!(
            replay.creation().agent.id,
            f.original.placement.target().agent_id
        );
        let fresh = f.trigger_reuse("after_closure", prompt).await;
        let next =
            f.f.f
                .original
                .resources
                .admit_agent_reusing_profile(fresh)
                .await;
        if let Ok(next) = next {
            assert_ne!(
                next.creation().agent.id,
                f.original.placement.target().agent_id,
                "new Trigger binding must not select the closed child"
            );
        }
        assert_eq!(
            f.row("agents", f.original.placement.target().agent_id.as_str())
                .await,
            before
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_closure_conflicts_with_stale_trigger_reuse_snapshot() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    let fresh = f.trigger_reuse("stale_snapshot", false).await;
    let parent = f.f.sandbox().await;
    let resources = &f.f.f.original.resources;
    let workspace = resources
        .get_workspace(&fresh.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut session = f.f.f.original.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    f.db()
        .collection::<Document>("agents")
        .find_one(doc! {"_id":f.original.placement.target().agent_id.as_str()})
        .session(&mut session)
        .await
        .unwrap();
    f.close_admission().await.unwrap();
    let result = crate::store::agent_admission::reuse_profile_agent_in_snapshot(
        resources,
        &fresh,
        &workspace,
        &parent,
        &mut session,
    )
    .await;
    assert!(
        result.is_err_and(|error| crate::store::sandbox_mongo::is_transient(&error)),
        "stale Trigger reuse must conflict with committed closure"
    );
    session.abort_transaction().await.unwrap();
    assert_eq!(
        f.db()
            .collection::<Document>("agent_reuse_bindings")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.close().await;
}
