use super::*;
use crate::store::{LifecycleBehavior, LifecycleInput};
use layer2_core::profiles::content_digest;

#[test]
fn captured_instructions_survive_replay_and_leave_submitted_request_unchanged() {
    for context in [None, Some("workspace-owned")] {
        let request = serde_json::from_value(serde_json::json!({
            "name":"Source work", "workspace_id":context,
            "profile":{"system_prompt":"Keep the user's exact instructions.\n"}
        }))
        .unwrap();
        let input = LifecycleInput {
            behavior: LifecycleBehavior::PromptRepository,
            managed_agent_instructions: Some(GITHUB_API_V1.into()),
            setup_message: None,
            request,
            resolved_image: None,
            capacity: None,
            capabilities: vec![],
            actor: None,
        };
        let submitted = serde_json::to_value(&input.request).unwrap();
        let effective =
            provisioning_request(&input.request, input.managed_agent_instructions.as_deref());
        assert_eq!(serde_json::to_value(&input.request).unwrap(), submitted);
        assert_eq!(effective.workspace_id, input.request.workspace_id);
        let prompt = effective
            .profile
            .as_ref()
            .unwrap()
            .system_prompt
            .as_ref()
            .unwrap();
        assert!(prompt.starts_with(GITHUB_API_V1));
        assert!(prompt.ends_with("Keep the user's exact instructions.\n"));
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
    let effective = provisioning_request(&request, Some(GITHUB_API_V1));
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
