//! Owner customization contracts replay real model/skill handlers and durable skill admission.
use std::sync::Arc;

use async_trait::async_trait;
use layer2_core::{
    profiles::{SkillRef, SkillSource},
    sandbox::{
        store::{AgentWorkMutation, AgentWorkerStore},
        AgentLogKind, AgentOperationService, AgentSkillInstallStatus, AgentStatus, AgentWorkStep,
        ResolvedAgentSkill, Sandbox, SandboxCredentialPrerequisite,
        SandboxCredentialPrerequisiteError,
    },
    Authorization, Capability, ServiceId, SkillId, SubjectId,
};

use super::{super::agents_tests as agents, Scenario};
use crate::rest::{
    agent_models_router, agent_router, AgentModelCatalog, AgentModelCatalogError,
    AgentModelRuntimeSupport, AgentModelView, AgentSkillRuntimeSupport,
};

struct Models(bool);

#[async_trait]
impl AgentModelCatalog for Models {
    async fn models(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        assert_eq!(sandbox.owner.as_str(), "owner-a");
        if self.0 {
            return Err(AgentModelCatalogError::Unavailable);
        }
        Ok(vec![AgentModelView {
            id: "provider-model".into(),
            label: "Provider model".into(),
            available: true,
        }])
    }
}

struct RuntimeSupport(bool, bool, bool);

#[async_trait]
impl AgentModelRuntimeSupport for RuntimeSupport {
    async fn checked_supported(
        &self,
        _: &Sandbox,
    ) -> Result<bool, layer2_core::sandbox::RuntimeCapacityError> {
        if self.2 {
            Err(layer2_core::sandbox::RuntimeCapacityError::Request)
        } else {
            Ok(self.0)
        }
    }
    async fn supported(&self, _: &Sandbox) -> bool {
        self.0
    }
}

#[async_trait]
impl AgentSkillRuntimeSupport for RuntimeSupport {
    async fn checked_supported(
        &self,
        _: &Sandbox,
        upgrade: bool,
    ) -> Result<bool, layer2_core::sandbox::RuntimeCapacityError> {
        if self.2 {
            Err(layer2_core::sandbox::RuntimeCapacityError::Request)
        } else {
            Ok(if upgrade { self.1 } else { self.0 })
        }
    }
    async fn supported(&self, _: &Sandbox) -> bool {
        self.0
    }
    async fn upgrade_supported(&self, _: &Sandbox) -> bool {
        self.1
    }
}

struct InstallDuringSupportCheck(Arc<AgentOperationService>);

#[async_trait]
impl AgentSkillRuntimeSupport for InstallDuringSupportCheck {
    async fn supported(&self, sandbox: &Sandbox) -> bool {
        self.0
            .install_skill(
                &layer2_core::Caller {
                    subject: sandbox.owner.clone(),
                    authorization: Authorization::new([Capability::UseOwnSandboxes]),
                },
                &layer2_core::AgentId("agt_contract".into()),
                SkillRef {
                    source: SkillSource::Ornn,
                    id: SkillId("research".into()),
                    version: Some("1.0".into()),
                },
            )
            .await
            .expect("competing skill installation is admitted during the runtime support check");
        true
    }
}

struct Connections(SandboxCredentialPrerequisiteError);

#[async_trait]
impl SandboxCredentialPrerequisite for Connections {
    async fn check(&self, _: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        Err(self.0.clone())
    }

    async fn check_sandbox(&self, _: &Sandbox) -> Result<(), SandboxCredentialPrerequisiteError> {
        Err(self.0.clone())
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("agent_customization_")?;
    let mut fixture = agents::fixture().await;
    fixture.state.agent_operations = fixture
        .state
        .agent_operations
        .with_skill_upgrades_enabled(mode != "upgrade_disabled");
    let agent = agents::seeded_agent(
        "agt_contract",
        if mode == "stopped" {
            AgentStatus::Stopped
        } else {
            AgentStatus::Ready
        },
        2,
        None,
    );
    let agent = agents::insert_agent(&fixture.store, agent, vec![]).await;
    if mode == "checkpoint" {
        super::experience_cases::begin_checkpoint(&fixture.store, &agent.sandbox_id).await;
    }
    if mode == "unavailable" {
        fixture.state.reads = super::fault_cases::unavailable_instance_reads();
    }
    let connection = match mode {
        "setup" => Some(SandboxCredentialPrerequisiteError::SetupRequired),
        "connections" => Some(SandboxCredentialPrerequisiteError::ConnectionsRequired {
            services: vec![ServiceId("api-github".into())],
        }),
        "connection_unavailable" => Some(SandboxCredentialPrerequisiteError::Unavailable(
            "contract connection authority unavailable".into(),
        )),
        _ => None,
    };
    if let Some(connection) = connection {
        fixture.state.agent_operations = fixture
            .state
            .agent_operations
            .with_connections(Arc::new(Connections(connection)));
    }
    let support = Arc::new(RuntimeSupport(
        mode != "unsupported",
        matches!(mode, "upgrade" | "upgrade_disabled" | "history_full"),
        mode == "runtime_capacity",
    ));
    if matches!(
        mode,
        "installed"
            | "failed"
            | "upgrade"
            | "upgraded"
            | "upgrade_failed"
            | "upgrade_disabled"
            | "upgrade_closed"
    ) {
        agents::skill_result_fixture::prepare_parents(&fixture.store, &agent).await;
        settle_skill(
            &fixture,
            if mode == "failed" {
                "failed"
            } else {
                "installed"
            },
            "1.0",
        )
        .await;
        if matches!(mode, "upgraded" | "upgrade_failed" | "upgrade_closed") {
            settle_skill(
                &fixture,
                if matches!(mode, "upgraded" | "upgrade_closed") {
                    "installed"
                } else {
                    "failed"
                },
                "1.1",
            )
            .await;
        }
    }
    if mode == "history_full" {
        agents::skill_result_fixture::prepare_parents(&fixture.store, &agent).await;
        for index in 0..128 {
            settle_skill(&fixture, "installed", &format!("v{index}")).await;
        }
    }
    if mode == "upgrade_closed" {
        fixture.state.agent_operations = fixture
            .state
            .agent_operations
            .with_skill_upgrades_enabled(false);
    }
    let models = agent_models_router(
        fixture.state.reads.clone(),
        Arc::new(Models(mode == "catalog_unavailable")),
        support.clone(),
    );
    let skill_support: Arc<dyn AgentSkillRuntimeSupport> = if mode == "install_race" {
        Arc::new(InstallDuringSupportCheck(
            fixture.state.agent_operations.clone(),
        ))
    } else {
        support
    };
    Some(Scenario {
        router: agent_router(fixture.state.with_skill_support(skill_support)).merge(models),
        subject: match mode {
            "anonymous" => None,
            "foreign" => Some("owner-b"),
            _ => Some("owner-a"),
        },
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::UseOwnSandboxes])
        },
        symbols: Default::default(),
    })
}

async fn settle_skill(fixture: &agents::Fixture, mode: &str, version: &str) {
    use std::time::{Duration, SystemTime};
    let caller = layer2_core::Caller {
        subject: SubjectId::new("owner-a"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let admitted = fixture
        .state
        .agent_operations
        .install_skill(
            &caller,
            &layer2_core::AgentId("agt_contract".into()),
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId("research".into()),
                version: Some(version.into()),
            },
        )
        .await
        .unwrap();
    let now = SystemTime::now() + Duration::from_secs(1);
    let stamp = layer2_core::rfc3339_at(now);
    let mut current = fixture
        .store
        .claim_agent_work(
            &admitted.id,
            "contract-skill-worker",
            stamp.clone(),
            layer2_core::rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    for pin_material in if mode == "installed" {
        vec![true, false]
    } else {
        vec![false]
    } {
        let mut next = current.clone();
        let mut logs = vec![];
        let item = next.work.live_skills.last_mut().unwrap();
        if pin_material {
            item.material = Some(ResolvedAgentSkill {
                source: SkillSource::Ornn,
                id: SkillId("research".into()),
                name: Some("Research".into()),
                version: version.into(),
                archive_sha256: "a".repeat(64),
            });
        } else {
            item.status = if mode == "installed" {
                AgentSkillInstallStatus::Installed
            } else {
                AgentSkillInstallStatus::Failed
            };
            item.error = (mode == "failed").then(|| "Skill archive could not be installed.".into());
            logs.push(AgentLogKind::SkillInstallationChanged {
                skill_id: "research".into(),
                status: item.status,
            });
            next.work.schedule(AgentWorkStep::None, stamp.clone());
        }
        next.updated_at = stamp.clone();
        next.revision += 1;
        next.conversation.last_seq += logs.len() as u64;
        let mutation = AgentWorkMutation {
            expected_agent_revision: current.revision,
            holder: "contract-skill-worker".into(),
            fence: current.work.lease.as_ref().unwrap().fence,
            agent: next.clone(),
            worktree: None,
            appended_at: stamp.clone(),
            sandbox_event: None,
            agent_logs: logs,
        };
        if pin_material {
            fixture.store.commit_agent_work(mutation).await
        } else {
            let guard = agents::skill_result_fixture::capture(&fixture.store, &current).await;
            fixture
                .store
                .commit_agent_skill_result(mutation, guard)
                .await
        }
        .unwrap();
        current = next;
    }
}
