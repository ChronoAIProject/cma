use super::attachments::runtime::UnusedRuntime;
use super::*;
use crate::{Authorization, Caller, Capability};

struct Resources;
#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("desktop authorization must not read resource usage")
    }
}

async fn desktop_fixture(
    machine: bool,
    isolated: bool,
    status: SandboxStatus,
    bound: bool,
    agent_status: AgentStatus,
) -> DesktopService {
    desktop_service(
        desktop_sandbox(machine, status, bound),
        isolated,
        agent_status,
    )
    .await
}

fn desktop_sandbox(machine: bool, status: SandboxStatus, bound: bool) -> Sandbox {
    let mut root = sandbox();
    root.status = status;
    if machine {
        root.profile.content.runtime_image = Some(format!("runtime@sha256:{}", "a".repeat(64)));
        root.profile
            .content
            .environment_variables
            .push(crate::profiles::EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            });
    }
    root.runtime = bound.then(|| RuntimeBinding {
        boot_id: None,
        runtime_id: "runtime-test".into(),
        runtime_version: "runtime-image".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    if bound {
        root.lifecycle.runtime_generation_high_water = 1;
    }
    root
}

async fn desktop_service(
    root: Sandbox,
    isolated: bool,
    agent_status: AgentStatus,
) -> DesktopService {
    let store = Arc::new(store::InMemorySandboxStore::default());
    store.create_sandbox(root).await.unwrap();
    store.create_workspace(workspace()).await.unwrap();
    store.create_worktree(worktree()).await.unwrap();
    let mut a = agent();
    a.status = agent_status;
    a.profile.content.isolated_agent = isolated;
    a.conversation.conversation_id = Some("conversation".into());
    a.conversation.runtime_generation = Some(1);
    a.work.step = AgentWorkStep::None;
    store.create_agent(a).await.unwrap();
    DesktopService::new(
        AgentReadService::new(store.clone(), Arc::new(Resources)).with_sessions(store),
        Arc::new(UnusedRuntime {
            status: None,
            file_root: None,
        }),
    )
}

#[tokio::test]
async fn desktop_uses_sandbox_configuration_without_changing_frozen_conversation_grants() {
    let service = desktop_fixture(true, false, SandboxStatus::Active, true, AgentStatus::Ready)
        .await
        .with_admission(DesktopAdmission::Protected);
    let owner = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let id = AgentId("agt_test".into());
    assert!(service.available(&owner, &id).await.is_ok());
    assert!(matches!(
        service.open(&owner, &id).await,
        Err(DesktopError::AuthorityUnavailable)
    ));
    let other = Caller::new(SubjectId("other".into()), owner.authorization.clone());
    assert!(matches!(
        service.available(&other, &id).await,
        Err(DesktopError::Unavailable)
    ));
    let denied = Caller::new(owner.subject, Authorization::new([]));
    assert!(matches!(
        service.available(&denied, &id).await,
        Err(DesktopError::Unavailable)
    ));
}

#[tokio::test]
async fn desktop_refuses_headless_isolated_paused_and_unbound_sandboxes() {
    let owner = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    for (machine, isolated, status, bound) in [
        (false, false, SandboxStatus::Active, true),
        (true, true, SandboxStatus::Active, true),
        (true, false, SandboxStatus::Paused, true),
        (true, false, SandboxStatus::Active, false),
    ] {
        let service = desktop_fixture(machine, isolated, status, bound, AgentStatus::Ready)
            .await
            .with_admission(DesktopAdmission::Protected);
        assert!(matches!(
            service.available(&owner, &AgentId("agt_test".into())).await,
            Err(DesktopError::Unavailable)
        ));
    }
}

#[tokio::test]
async fn desktop_startup_requires_an_active_owned_machine_and_refuses_terminal_agents() {
    let owner = Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let other = Caller::new(SubjectId("other".into()), owner.authorization.clone());
    let id = AgentId("agt_test".into());
    for sandbox_status in [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Reviving,
        SandboxStatus::Paused,
    ] {
        for agent_status in [
            AgentStatus::Starting,
            AgentStatus::Ready,
            AgentStatus::Working,
            AgentStatus::WaitingApproval,
            AgentStatus::Paused,
            AgentStatus::Stopped,
            AgentStatus::Failed,
            AgentStatus::Deleted,
        ] {
            let service = desktop_fixture(true, false, sandbox_status, true, agent_status)
                .await
                .with_admission(DesktopAdmission::Protected);
            let expected = sandbox_status == SandboxStatus::Active
                && matches!(
                    agent_status,
                    AgentStatus::Starting
                        | AgentStatus::Ready
                        | AgentStatus::Working
                        | AgentStatus::WaitingApproval
                );
            assert_eq!(service.available(&owner, &id).await.is_ok(), expected);
            assert!(service.available(&other, &id).await.is_err());
        }
    }
}

#[path = "desktop/admission_tests.rs"]
mod admission;
