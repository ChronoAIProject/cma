use super::*;
use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::{AgentSkill, AgentSkillInstallRequest, AgentSlot, SandboxAdapter as _, SandboxSpec};

fn skill(version: &str) -> AgentSkill {
    AgentSkill {
        id: "research".into(),
        name: Some("Research".into()),
        version: Some(version.into()),
        archive_sha256: "a".repeat(64),
        source_url: "https://skills.example/archive".into(),
    }
}

#[tokio::test]
async fn profile_upgrade_uses_archive_identity_after_provider_drops_display_name() {
    for mismatch in [None, Some("id"), Some("version"), Some("digest")] {
        let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
        let handle = adapter
            .provision(&SandboxSpec {
                sandbox_id: "skill-fixture".into(),
                ..SandboxSpec::default()
            })
            .await
            .unwrap();
        let slot = AgentSlot::new("agent");
        let mut installed = skill("1.0");
        // Codex uses the name to select fallbacks, then omits it from the home plan.
        installed.name = None;
        let home = AgentHomePlan {
            operation_id: "agent:agent:home:create:v1".into(),
            request_sha256: "b".repeat(64),
            agent_id: slot.clone(),
            files: vec![],
            symlinks: vec![],
            skills: vec![installed],
        };
        let mut runtimes = backend.runtimes.lock().await;
        let runtime = runtimes.get_mut(&handle).unwrap();
        runtime.homes.insert(slot.clone(), home.clone());
        let mut previous = skill("1.0");
        match mismatch {
            Some("id") => previous.id = "different".into(),
            Some("version") => previous.version = Some("9.0".into()),
            Some("digest") => previous.archive_sha256 = "c".repeat(64),
            None => {}
            _ => unreachable!(),
        }
        let request =
            AgentSkillInstallRequest::upgrade(slot.clone(), skill("1.1"), previous, None, 1);
        let result = install_skill(runtime, FencedRequest::new(1, request.clone()));
        if mismatch.is_some() {
            assert!(matches!(
                result,
                Err(crate::AdapterError::OperationIdentityConflict(_))
            ));
            assert_eq!(runtime.homes[&slot], home);
        } else {
            result.expect("display names do not change the installed archive identity");
            assert_eq!(runtime.homes[&slot].skills, vec![skill("1.1")]);
            install_skill(runtime, FencedRequest::new(2, request)).unwrap();
            assert_eq!(runtime.homes[&slot].skills, vec![skill("1.1")]);
        }
    }
}
