//! Start with the actual preparation engine, then replay the owned home request.
use super::*;
use layer3_adapter::{AgentHomeRequest, AgentOpenRequest, AgentSlot, WorktreeRequest};

#[tokio::test]
async fn native_prepared_guidance_home_survives_owned_replay_without_identity_conflict() {
    for prompt in [
        crate::agent_lifecycle::guidance::managed_v3(),
        crate::agent_lifecycle::guidance::managed_v2(),
        crate::agent_lifecycle::guidance::GITHUB_API_V1,
        "",
    ] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let pools = pools(&harness, &[]);
        let mut initial = reservation(&harness).await;
        let profile = &mut initial.plan.workspaces[0].agents[0];
        profile.content.system_prompt = prompt.into();
        profile.digest = content_digest(&profile.content).unwrap();
        let prepared = ready(&harness, &pools, initial).await;
        let workspace = &prepared.workspaces[0];
        let agent = &workspace.agents[0];
        let handle = RuntimeHandle::new(&prepared.runtime.as_ref().unwrap().runtime_id);
        let runtime_adapter = adapter(&harness, pools);
        let verified = prepare_step(
            runtime_adapter.as_ref(),
            "test",
            &prepared,
            &home_tests::home_policy(),
        )
        .await
        .unwrap();
        assert_eq!(verified, prepared);
        let options = crate::sandbox::runtime_mapping::profile_runtime_options(
            &prepared.plan.workspaces[0].agents[0].content,
            &prepared.plan.profile.content,
            &agent.id,
            "/home/cma/workspace/.worktrees/wt-1",
        );
        let request = AgentHomeRequest::new(
            format!("agent:{}:home:create:v1", agent.id),
            options.clone(),
            prompt,
            Vec::new(),
        );
        let before = harness.backend.snapshot(&handle).await.unwrap();
        let home = &before.homes[&AgentSlot(agent.id.to_string())];
        assert_eq!(
            home.files
                .iter()
                .find(|file| file.path == "AGENTS.md")
                .unwrap()
                .bytes,
            prompt.as_bytes(),
            "the actual prepared home must contain the admitted profile bytes"
        );
        // These are the same operation identities replayed by the owned worker,
        // with the later fence assigned during prepared ownership transfer.
        runtime_adapter
            .add_worktree(
                &handle,
                2,
                &WorktreeRequest::new(
                    "worktree:workspace:wt-1:add:v1",
                    "workspace",
                    "wt-1",
                    workspace.resolved_commit.clone().unwrap(),
                ),
            )
            .await
            .unwrap();
        runtime_adapter
            .materialize_agent_home(&handle, 2, &request)
            .await
            .unwrap();
        let (sender, _receiver) = tokio::sync::mpsc::channel(256);
        let opened = runtime_adapter
            .reconnect_agent(
                &handle,
                2,
                &AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None),
                agent.conversation_id.as_deref().unwrap(),
                sender,
            )
            .await
            .unwrap();
        assert_eq!(opened.conversation_id, agent.conversation_id);
        let after = harness.backend.snapshot(&handle).await.unwrap();
        assert_eq!(after.homes, before.homes);
        assert_eq!(after.worktrees, before.worktrees);
        assert_eq!(after.agents, before.agents);
        assert!(!after.credential_delivered);
        assert!(harness.actions.snapshot().is_empty());
    }
}
