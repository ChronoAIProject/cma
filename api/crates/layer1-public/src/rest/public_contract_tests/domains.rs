use layer2_core::profiles::store::{AgentProfileStore, WorkspaceProfileStore};
use layer2_core::profiles::{ProfileVisibility, WorkspaceLocation};
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::AgentStatus;
use layer2_core::{Authorization, Capability, WorkspaceProfileId};

use super::super::{
    admin::tests as admin, agents_tests as agents, instances_tests as instances,
    profiles_tests as profiles,
};
use super::Scenario;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let (router, subject, capabilities) = match name {
        "diagnostic_report" => (super::super::browser_telemetry_router(|_| {}), None, vec![]),
        "discovery" => (super::super::router(), None, vec![]),
        "profiles_owner" | "profiles_forbidden" | "profiles_anonymous" | "marketplace_disabled" => {
            (
                profiles::fixture(name != "marketplace_disabled"),
                (name != "profiles_anonymous").then_some("contract-owner"),
                if name == "profiles_forbidden" {
                    vec![]
                } else {
                    vec![Capability::ManageOwnProfiles, Capability::UseOwnSandboxes]
                },
            )
        }
        "instances_owner"
        | "instances_forbidden"
        | "instances_anonymous"
        | "instances_quota"
        | "instances_paused"
        | "instances_not_paused"
        | "instances_corrupt"
        | "instances_trust"
        | "instances_capacity"
        | "instances_invalid_workspace_name"
        | "instances_lifecycle" => {
            let fixture = instances::fixture(if name == "instances_quota" { 1 } else { 10 });
            let (profile_owner, visibility, commands) = if name == "instances_trust" {
                (
                    "publisher",
                    ProfileVisibility::Public,
                    vec!["printf proof".into()],
                )
            } else {
                ("owner-a", ProfileVisibility::Private, vec![])
            };
            instances::publish_sandbox_profile(
                &fixture.profiles,
                "sbp_contract",
                profile_owner,
                visibility,
                commands.clone(),
            )
            .await;
            instances::publish_workspace_profile(
                &fixture.profiles,
                "wkp_contract",
                profile_owner,
                visibility,
                "contract-project",
                commands,
            )
            .await;
            if name == "instances_invalid_workspace_name" {
                let id = WorkspaceProfileId("wkp_contract".into());
                let mut profile = fixture
                    .profiles
                    .workspace_profile(&id)
                    .await
                    .unwrap()
                    .unwrap();
                profile.draft.location = WorkspaceLocation::GitRepository {
                    url: "https://github.com/contract/.git".into(),
                    reference: None,
                };
                profile.draft.name.clear();
                profile.draft_version = 2;
                fixture
                    .profiles
                    .save_workspace_draft(1, profile)
                    .await
                    .unwrap();
                fixture
                    .profiles
                    .publish_workspace_profile(&id, 2, "2026-09-05T00:00:00Z".into())
                    .await
                    .unwrap();
            }
            let mut sandbox = if name == "instances_paused" {
                instances::paused_sandbox("sbx_route", "owner-a")
            } else {
                agents::sandbox("sbx_route", "owner-a")
            };
            if name == "instances_corrupt" {
                sandbox.name.clear();
            }
            if name == "instances_not_paused" {
                sandbox
                    .begin_retirement("2026-09-05T00:00:00Z".into())
                    .unwrap();
                sandbox.revision = 1;
            }
            if name == "instances_capacity" {
                sandbox.limits.max_workspaces = 1;
            }
            fixture.store.create_sandbox(sandbox).await.unwrap();
            fixture
                .store
                .create_workspace(agents::workspace("wks_route", "sbx_route", "owner-a"))
                .await
                .unwrap();
            let agent = agents::seeded_agent("agt_contract", AgentStatus::Ready, 0, None);
            fixture
                .store
                .create_worktree(agents::seeded_worktree(&agent))
                .await
                .unwrap();
            fixture.store.create_agent(agent).await.unwrap();
            (
                fixture.router,
                (name != "instances_anonymous").then_some("owner-a"),
                if name == "instances_forbidden" {
                    vec![]
                } else {
                    vec![
                        Capability::CreateSandbox,
                        Capability::UseOwnSandboxes,
                        Capability::DeleteOwnSandbox,
                    ]
                },
            )
        }
        "agents_owner" | "agents_forbidden" | "agents_anonymous" | "agents_stopped" => {
            let fixture = agents::fixture().await;
            let status = if name == "agents_stopped" {
                AgentStatus::Stopped
            } else {
                AgentStatus::Ready
            };
            let agent = agents::seeded_agent("agt_contract", status, 2, None);
            agents::insert_agent(&fixture.store, agent, vec![]).await;
            (
                fixture.router,
                (name != "agents_anonymous").then_some("owner-a"),
                if name == "agents_forbidden" {
                    vec![]
                } else {
                    vec![Capability::UseOwnSandboxes]
                },
            )
        }
        "admin_operator" | "admin_forbidden" | "admin_anonymous" | "admin_no_inventory"
        | "admin_actor_search" => {
            let fixture = admin::fixture(name != "admin_no_inventory");
            let actor_search = name == "admin_actor_search";
            fixture
                .names
                .searches_enabled
                .store(actor_search, std::sync::atomic::Ordering::SeqCst);
            fixture
                .sandboxes
                .create_sandbox(admin::sandbox("sbx_contract", "owner-a", "runtime-v1"))
                .await
                .unwrap();
            for id in ["agp_contract", layer2_core::DEFAULT_AGENT_PROFILE_ID] {
                fixture
                    .profiles
                    .create_agent_profile(admin::agent_profile(
                        id,
                        "owner-a",
                        ProfileVisibility::Public,
                    ))
                    .await
                    .unwrap();
            }
            (
                fixture.app,
                (name != "admin_anonymous").then_some("operator-a"),
                if name == "admin_forbidden" {
                    vec![]
                } else if actor_search {
                    vec![Capability::ReadActivityLog, Capability::ReadUserDirectory]
                } else {
                    vec![
                        Capability::ConfigurePlatform,
                        Capability::ReadActivityLog,
                        Capability::ReadAllSandboxes,
                        Capability::ControlAnySandbox,
                        Capability::ReadEventLog,
                        Capability::ReadRuntimeInventory,
                    ]
                },
            )
        }
        _ => return None,
    };
    Some(Scenario {
        router,
        subject,
        authorization: Authorization::new(capabilities),
        symbols: Default::default(),
    })
}
