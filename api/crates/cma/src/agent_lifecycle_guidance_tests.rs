use super::*;
use crate::store::{LifecycleBehavior, LifecycleInput};
use layer2_core::profiles::content_digest;
use layer2_core::sandbox::{AgentInstructionPolicy, AgentSettings, AgentStatus};

#[test]
fn owner_edits_preserve_exact_managed_revision_and_owner_whitespace() {
    for managed in [
        GITHUB_API_V1,
        managed_v2(),
        managed_v3(),
        managed_v4(),
        managed_v5(),
    ] {
        let owner = "  Keep citations.\n\nDo not change my formatting.\n";
        let original = compose_managed(Some(managed), owner);
        assert_eq!(split_managed(&original), (Some(managed), owner));
        assert_eq!(split_managed(managed), (Some(managed), ""));
        let replacement = compose_managed(split_managed(&original).0, "New owner instructions");
        assert_eq!(replacement, format!("{managed}\n\nNew owner instructions"));
        assert_eq!(compose_managed(split_managed(&original).0, ""), managed);
    }
    for owner in ["", "CMA GitHub workflow:\nUser-authored text", "  owner\n"] {
        assert_eq!(split_managed(owner), (None, owner));
        assert_eq!(compose_managed(None, owner), owner);
    }
}

#[test]
fn owner_policy_uses_the_captured_revision_even_after_transport_refresh() {
    let policy = ManagedInstructionPolicy;
    let mut agent = crate::auth::test_resources::agent(
        "agent",
        "worktree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Ready,
    );
    let owner = format!("{SESSION_GUIDANCE_V1}\n\n  owner continuation\n");
    agent.profile.content.system_prompt = compose_managed(Some(managed_v3()), &owner);
    assert_eq!(policy.owner_prompt(&agent), owner);
    assert_eq!(policy.pinned_managed_guidance(&agent), None);
    assert_eq!(policy.compose(&agent, &owner), agent.system_prompt());

    let owner = "  Keep the exact owner text.\n";
    let original = compose_managed(Some(managed_v4()), owner);
    agent.profile.content.system_prompt = original.clone();
    agent.conversation.managed_guidance_revision = 2;
    agent.conversation.settings = Some(AgentSettings {
        system_prompt: Some(original),
        ..agent.settings()
    });
    assert_eq!(policy.owner_prompt(&agent), owner);
    assert_eq!(policy.pinned_managed_guidance(&agent), Some(managed_v4()));
    let edited = policy.compose(&agent, "  New owner text\n");
    assert_eq!(edited, format!("{}\n\n  New owner text\n", managed_v4()));
    assert_eq!(edited.matches(SESSION_GUIDANCE_V1).count(), 1);
    assert_eq!(edited.matches(managed_v3()).count(), 1);

    let owner = format!("{GITHUB_PERMISSION_V1}\n\n  exact owner text\n");
    agent.profile.content.system_prompt = compose_managed(Some(managed_v5()), &owner);
    agent.conversation.settings = Some(AgentSettings {
        system_prompt: Some(agent.profile.content.system_prompt.clone()),
        ..agent.settings()
    });
    assert_eq!(policy.owner_prompt(&agent), owner);
    assert_eq!(policy.pinned_managed_guidance(&agent), Some(managed_v5()));
    let edited = policy.compose(&agent, "  New owner text\n");
    assert_eq!(edited, format!("{}\n\n  New owner text\n", managed_v5()));
    assert_eq!(edited.matches(GITHUB_PERMISSION_V1).count(), 1);
    assert_eq!(edited.matches(managed_v4()).count(), 1);
}

#[test]
fn new_admission_guidance_requires_explicit_activation() {
    const V4: &str = "CHRONO_AGENT_SESSION_GUIDANCE_V4_ENABLED";
    const V5: &str = "CHRONO_AGENT_SESSION_GUIDANCE_V5_ENABLED";
    for (v4, v5, expected) in [
        (None, None, managed_v3()),
        (Some("false"), Some("false"), managed_v3()),
        (Some("true"), Some("false"), managed_v4()),
        (None, Some("true"), managed_v5()),
        (Some("true"), Some("true"), managed_v5()),
    ] {
        let flags = crate::config::FeatureFlags::from_lookup(&mut |key| {
            Ok(match key {
                V4 => v4.map(str::to_owned),
                V5 => v5.map(str::to_owned),
                _ => None,
            })
        })
        .unwrap();
        assert_eq!(
            admission_instructions(flags.session_guidance_v4, flags.session_guidance_v5),
            expected
        );
    }
    for flag in [V4, V5] {
        let error = crate::config::FeatureFlags::from_lookup(&mut |key| {
            Ok((key == flag).then(|| "TRUE".to_owned()))
        })
        .unwrap_err();
        assert!(error.contains(flag));
    }
}

#[test]
fn feedback_guidance_checks_the_target_before_recommending_github_scope() {
    let guidance = managed_v5();
    for required in [
        "For CMA feedback, the target is ChronoAIProject/cma.",
        "verify the target repository's current visibility and issue policy",
        "When the target is public, classic GitHub OAuth public_repo can suffice",
        "For an actual private or internal target, classic OAuth requires repo",
        "limited to the intended repository with Issues write permission",
        "Treat anything submitted there as public and redact it",
        "owner's authorization to file",
    ] {
        assert!(guidance.contains(required), "missing guidance: {required}");
    }
    assert!(!guidance.contains("the internal ChronoAIProject/cma repository"));
}

#[test]
fn captured_instructions_survive_replay_and_leave_submitted_request_unchanged() {
    for instructions in [
        GITHUB_API_V1,
        managed_v2(),
        managed_v3(),
        managed_v4(),
        managed_v5(),
    ] {
        for (sandbox, workspace) in [
            (None, None),
            (Some("sandbox-owned"), None),
            (None, Some("workspace-owned")),
        ] {
            let request = serde_json::from_value(serde_json::json!({
                "name":"Source work", "workspace_id":workspace, "sandbox_id":sandbox,
                "profile":{"system_prompt":"Keep the user's exact instructions.\n"}
            }))
            .unwrap();
            let input = LifecycleInput {
                behavior: LifecycleBehavior::PromptRepository,
                managed_agent_instructions: Some(instructions.into()),
                setup_message: None,
                request,
                resolved_image: None,
                capacity: None,
                sandbox_availability: Default::default(),
                capabilities: vec![],
                actor: None,
            };
            let submitted = serde_json::to_value(&input.request).unwrap();
            let effective =
                provisioning_request(&input.request, input.managed_agent_instructions.as_deref());
            assert_eq!(serde_json::to_value(&input.request).unwrap(), submitted);
            assert_eq!(effective.workspace_id, input.request.workspace_id);
            assert_eq!(effective.sandbox_id, input.request.sandbox_id);
            let prompt = effective
                .profile
                .as_ref()
                .unwrap()
                .system_prompt
                .as_ref()
                .unwrap();
            assert_eq!(
                prompt,
                &format!("{instructions}\n\nKeep the user's exact instructions.\n")
            );
            assert_eq!(prompt.contains(MEMORY_V1), instructions != GITHUB_API_V1);
            assert_eq!(
                prompt.contains(DOCUMENTS_V1),
                [managed_v3(), managed_v4(), managed_v5()].contains(&instructions)
            );
            assert_eq!(prompt.matches("CMA GitHub workflow:").count(), 1);
            let restored: LifecycleInput =
                serde_json::from_slice(&serde_json::to_vec(&input).unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(provisioning_request(
                    &restored.request,
                    restored.managed_agent_instructions.as_deref(),
                ))
                .unwrap(),
                serde_json::to_value(effective).unwrap()
            );
        }
    }
}

#[test]
fn retained_operations_preserve_their_original_profile_and_home_payload() {
    for behavior in ["legacy", "prompt_repository"] {
        let old: LifecycleInput = serde_json::from_value(serde_json::json!({
            "behavior":behavior,
            "request":{"name":"Retained","profile":{"system_prompt":"Original instructions"}},
            "resolved_image":null
        }))
        .unwrap();
        assert!(old.managed_agent_instructions.is_none());
        let before = crate::agent_lifecycle::validation::agent_content_for(
            old.request.profile.as_ref().unwrap(),
            old.behavior,
        )
        .unwrap();
        let effective = provisioning_request(&old.request, None);
        let after = crate::agent_lifecycle::validation::agent_content_for(
            effective.profile.as_ref().unwrap(),
            old.behavior,
        )
        .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            content_digest(&before).unwrap(),
            content_digest(&after).unwrap()
        );
        let home = |prompt: String| {
            let request = layer3_adapter::AgentHomeRequest::new(
                "agent:retained:home:create:v1",
                layer3_adapter::RuntimeOptions {
                    slot: layer3_adapter::AgentSlot("retained".into()),
                    home_directory: "/home/cma/.cma/agents/retained/home".into(),
                    working_directory: "/home/cma/workspace/.worktrees/wt-1".into(),
                    ..Default::default()
                },
                prompt,
                vec![],
            );
            layer3_adapter::codex::render_agent_home(&request, &Default::default()).unwrap()
        };
        assert_eq!(
            home(before.system_prompt).payload_sha256(),
            home(after.system_prompt).payload_sha256()
        );
    }
}

#[test]
fn new_default_profile_matches_preparation_and_rejects_custom_prepared_instructions() {
    let request = serde_json::from_value(serde_json::json!({"name":"Default"})).unwrap();
    let effective = provisioning_request(&request, Some(managed_v3()));
    let profile = crate::agent_lifecycle::validation::agent_content_for(
        effective.profile.as_ref().unwrap(),
        LifecycleBehavior::PromptRepository,
    )
    .unwrap();
    assert_eq!(
        profile,
        crate::agent_lifecycle::prepared_default_agent_profile().unwrap()
    );
    assert!(is_prepared_prompt(&profile.system_prompt));
    assert!(is_prepared_prompt(""));
    assert!(is_prepared_prompt(GITHUB_API_V1));
    assert_eq!(profile.system_prompt, managed_v3());
    assert!(!is_prepared_prompt(&format!(
        "{}\nUser-specific text",
        profile.system_prompt
    )));
    assert!(profile
        .system_prompt
        .contains("initialize a local repository"));
    assert!(profile.system_prompt.contains("default_branch"));
    assert!(profile.system_prompt.contains("tarball/{sha}"));
    assert!(profile
        .system_prompt
        .contains("only if the archive is unavailable"));
    assert!(profile
        .system_prompt
        .contains("POST /user/repos or /orgs/{org}/repos"));
}

#[test]
fn current_guidance_preserves_github_bytes_and_bounds_memory_management() {
    use sha2::{Digest, Sha256};
    // This revision is already persisted in live profiles and home receipts.
    assert_eq!(
        format!("{:x}", Sha256::digest(GITHUB_API_V1)),
        "51ed3e4469377a625715bc15a4fb0273b4dbe6a3942570bdffe80fa0a002bd25"
    );
    assert_eq!(
        managed_v2().strip_prefix(GITHUB_API_V1),
        Some(format!("\n\n{MEMORY_V1}").as_str())
    );
    assert_eq!(
        managed_v3().strip_prefix(managed_v2()),
        Some(format!("\n\n{DOCUMENTS_V1}").as_str())
    );
    for (guidance, hash) in [
        (
            managed_v2(),
            "00825da641276488015e6e08a732c36a507ad6a3668fa854529f5d5c15bc5531",
        ),
        (
            managed_v3(),
            "8cd8800d0a69f4d1416ddb63a81ec7328bcd6c55d24bd5d3e43ab6f6684e8884",
        ),
        (
            managed_v4(),
            "d9432645d2f47ff0ebd093f0ad6c5eda1ea631e38a64a46e9289606f6ec6f5e3",
        ),
        (
            managed_v5(),
            "c880d275b266d34db85da31ac0189fb017bcd5aa3d645ad023159c07267c2e40",
        ),
    ] {
        assert_eq!(format!("{:x}", Sha256::digest(guidance)), hash);
    }
    for required in [
        "effective memory limits, total usage",
        "cgroup limits/usage",
        "shared by its runtime, other Agents and processes",
        "Reserve headroom",
        "bounded worker/job concurrency",
        "streaming or out-of-core",
        "sensible intervals or between stages",
        "stop only your own unnecessary jobs",
        "Never kill another Agent",
        "delete user files or workspace state",
        "privileged kernel cache-drop",
        "Do not blindly retry",
        "do not claim continuous monitoring outside active turns",
    ] {
        assert!(
            MEMORY_V1.contains(required),
            "missing constraint: {required}"
        );
    }
}
