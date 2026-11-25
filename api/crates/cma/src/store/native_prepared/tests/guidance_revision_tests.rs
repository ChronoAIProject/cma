use super::*;

fn historical(mut plan: NativePreparedPlan, revision: u32, prompt: &str) -> NativePreparedPlan {
    let workspace = &mut plan.warm.workspaces[0];
    workspace.agents[0].revision = revision;
    workspace.agents[0].content.system_prompt = prompt.into();
    workspace.agents[0].digest = content_digest(&workspace.agents[0].content).unwrap();
    workspace.profile.content.agent_profiles[0].revision = revision;
    workspace.profile.digest = content_digest(&workspace.profile.content).unwrap();
    plan.validate().unwrap();
    plan
}

#[test]
fn native_prepared_guidance_revision_reads_old_plans_without_rewriting_their_identity() {
    let current = plan();
    assert_eq!(
        current.warm.workspaces[0].agents[0].revision,
        model::PREPARED_AGENT_REVISION
    );
    assert_eq!(
        current.warm.workspaces[0].agents[0].content.system_prompt,
        crate::agent_lifecycle::guidance::managed_v3()
    );
    assert!(current.renders_current_home());
    for (revision, prompt) in [
        (1, ""),
        (2, crate::agent_lifecycle::guidance::GITHUB_API_V1),
        (3, crate::agent_lifecycle::guidance::managed_v2()),
    ] {
        let old = historical(current.clone(), revision, prompt);
        let decoded: NativePreparedPlan =
            serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, old);
        assert_ne!(decoded, current);
        assert!(!decoded.renders_current_home());
        let (sandbox, workspace) = actual_profiles(&old);
        assert!(old.matches(
            &sandbox,
            &workspace,
            &old.warm.workspaces[0].agents[0].content,
            &[]
        ));
        assert!(!current.matches(
            &sandbox,
            &workspace,
            &old.warm.workspaces[0].agents[0].content,
            &[]
        ));
    }
}

/// SHA-256 of the prepared Agent home `config.toml` under
/// `model::PREPARED_AGENT_REVISION`. If this changes, the prepared home bytes
/// changed: give the plan a new revision and pin the new digest together.
const PREPARED_HOME_CONFIG_SHA256: &str =
    "0d4132f7a3bd334163df3990f72c75758b002c2e641f60e3dda3e4042f2f9dcf";

fn prepared_config(plan: &NativePreparedPlan) -> String {
    let agent = &plan.warm.workspaces[0].agents[0].content;
    let options = layer3_adapter::RuntimeOptions {
        model: agent.model.clone(),
        reasoning_effort: match agent.thinking_effort {
            ThinkingEffort::Minimal => layer3_adapter::ReasoningEffort::Minimal,
            ThinkingEffort::Low => layer3_adapter::ReasoningEffort::Low,
            ThinkingEffort::Medium => layer3_adapter::ReasoningEffort::Medium,
            ThinkingEffort::High => layer3_adapter::ReasoningEffort::High,
            ThinkingEffort::XHigh => layer3_adapter::ReasoningEffort::XHigh,
        },
        permission: match agent.permission_level {
            PermissionLevel::Guarded => layer3_adapter::PermissionLevel::Guarded,
            PermissionLevel::Autonomous => layer3_adapter::PermissionLevel::Autonomous,
        },
        // A prepared Sandbox always uses ChronoAI-managed model access.
        own_credential: false,
        isolated_agent: agent.isolated_agent,
        working_directory: "/home/cma/workspace/.worktrees/wt-1".into(),
        home_directory: "/home/cma/.cma/agents/agent-prepared/home".into(),
        slot: layer3_adapter::AgentSlot::new("agent-prepared".to_string()),
    };
    layer3_adapter::codex::render_config(&options, &layer3_adapter::codex::CodexConfig::default())
        .unwrap()
}

#[test]
fn native_prepared_home_bytes_are_pinned_to_their_revision() {
    use sha2::Digest;
    let current = plan();
    let config = prepared_config(&current);
    assert!(config.contains("stream_max_retries = 1"));
    let digest = sha2::Sha256::digest(config.as_bytes());
    let digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        (
            current.warm.workspaces[0].agents[0].revision,
            digest.as_str()
        ),
        (model::PREPARED_AGENT_REVISION, PREPARED_HOME_CONFIG_SHA256),
        "prepared home bytes changed: bump PREPARED_AGENT_REVISION and pin the new digest"
    );
}

#[test]
fn native_prepared_home_from_an_earlier_renderer_is_never_claimable() {
    let current = plan();
    // Revision 4 had today's guidance and content but rendered implicit
    // Codex stream limits. The owner's request still matches its content,
    // so only the rendering identity keeps it from being replayed.
    let old = historical(
        current.clone(),
        4,
        crate::agent_lifecycle::guidance::managed_v3(),
    );
    let decoded: NativePreparedPlan =
        serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, old);
    assert_ne!(decoded, current);
    let (sandbox, workspace) = actual_profiles(&old);
    assert!(current.matches(
        &sandbox,
        &workspace,
        &old.warm.workspaces[0].agents[0].content,
        &[]
    ));
    assert!(!decoded.renders_current_home());
    assert!(current.renders_current_home());
    let mut forged = current.clone();
    forged.warm.workspaces[0].agents[0].content.system_prompt =
        crate::agent_lifecycle::guidance::managed_v2().into();
    assert!(!forged.renders_current_home());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_guidance_revision_replaces_only_unclaimed_older_homes() {
    for (revision, prompt) in [
        (1, ""),
        (2, crate::agent_lifecycle::guidance::GITHUB_API_V1),
        (3, crate::agent_lifecycle::guidance::managed_v2()),
        (4, crate::agent_lifecycle::guidance::managed_v3()),
    ] {
        let (client, name, store) = database().await;
        let current = plan();
        let old = historical(current.clone(), revision, prompt);
        assert_ne!(old, current);
        // An earlier process's target and spare, before this process's first
        // preparation sweep replaces them.
        store
            .reconcile_native_prepared_target(Some(&old), 1, NOW)
            .await
            .unwrap();
        let spare = publish_ready(&store).await;
        let request = admission(&store, &old, "guidance-owner", "guidance-fallback").await;
        let mut session = store.client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let outcome = super::super::admission::claim_ready(
            &store,
            &mut request.sandbox.clone(),
            NOW,
            &mut session,
        )
        .await
        .map_err(|_| ())
        .unwrap();
        session.abort_transaction().await.unwrap();
        assert_eq!(
            outcome,
            ClaimOutcome::Miss("miss_prepared_home_rendering"),
            "revision {revision}"
        );
        let SandboxAdmissionResult::Created(fallback) =
            admit_at(&store, request, NOW).await.unwrap()
        else {
            panic!("fresh admission must continue through ordinary provisioning");
        };
        assert!(fallback.runtime.is_none());
        assert_ne!(fallback.id, spare.payload.id);
        assert_eq!(
            store
                .native_prepared_record(&spare.payload.id)
                .await
                .unwrap()
                .unwrap(),
            spare
        );

        store
            .reconcile_native_prepared_target(Some(&current), 1, NOW)
            .await
            .unwrap();
        let next = records(&store).await;
        assert_eq!(
            next.iter()
                .find(|row| row.payload.id == spare.payload.id)
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
        assert_ne!(replacement.payload.id, spare.payload.id);
        assert!(replacement.ownership.is_none());

        // A current home claimed by this process survives an older
        // process's sweep during a rolling overlap or rollback.
        let ready = publish_ready(&store).await;
        let input = admission(&store, &current, "guidance-owner", "guidance-owned").await;
        let SandboxAdmissionResult::Created(owned) = admit_at(&store, input, NOW).await.unwrap()
        else {
            panic!("fresh owner admission expected");
        };
        assert_eq!(owned.id, ready.payload.id);
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
        assert_eq!(
            records(&store)
                .await
                .into_iter()
                .find(|row| row.payload.id == owned.id)
                .unwrap(),
            claimed
        );
        assert_eq!(store.get_sandbox(&owned.id).await.unwrap().unwrap(), owned);
        client.database(&name).drop().await.unwrap();
    }
}
