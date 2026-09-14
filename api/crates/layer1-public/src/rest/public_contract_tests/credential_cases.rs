//! Deterministic admission-state fixtures; no OAuth exchange is simulated here.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use layer2_core::profiles::{
    AgentProfileRef, InMemoryProfileStore, ProfileVisibility, SandboxProfileRef,
    WorkspaceProfileRef,
};
use layer2_core::sandbox::store::InMemorySandboxStore;
use layer2_core::sandbox::{
    AgentContextService, AgentService, FixedSandboxCredentialPrerequisite, HierarchyReadService,
    SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError, SandboxPolicy,
    SandboxService, WorkspaceService,
};
use layer2_core::trigger_domain::ServiceRequirementState;
use layer2_core::{
    AgentProfileId, Authorization, Capability, PlatformDefaults, SandboxProfileId, SubjectId,
    WorkspaceProfileId,
};
use serde_json::json;

use super::super::trigger_routes_tests::support_tests::{
    create_trigger, json_response, publish_trigger, visitor_authority, visitor_request, TestApp,
    VISITOR,
};
use super::super::{
    agents_tests as agents, hierarchy_router, instances_tests as instances, HierarchyState,
};
use super::Scenario;

struct RecoveringPrerequisite {
    owner: SubjectId,
    refuse_next: AtomicBool,
}

#[async_trait::async_trait]
impl SandboxCredentialPrerequisite for RecoveringPrerequisite {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        assert_eq!(owner, &self.owner);
        // Model out-of-band setup becoming available after one definitive refusal.
        if self.refuse_next.swap(false, Ordering::SeqCst) {
            Err(SandboxCredentialPrerequisiteError::SetupRequired)
        } else {
            Ok(())
        }
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    match name {
        "credential_sandbox" => Some(sandbox().await),
        "credential_conversation" => Some(conversation().await),
        "credential_launch_create" | "credential_launch_start" => Some(launch(name).await),
        _ => None,
    }
}

async fn conversation() -> Scenario {
    let (mut state, _) = agents::conversation_fixture().await;
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    agents::publish_profile(&profiles).await;
    instances::publish_sandbox_profile(
        &profiles,
        "sbp_contract",
        "owner-a",
        ProfileVisibility::Private,
        vec![],
    )
    .await;
    let sandboxes = SandboxService::new(
        store.clone(),
        profiles.clone(),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
    );
    let workspaces = WorkspaceService::new(store.clone(), profiles.clone());
    state.context = AgentContextService::new(
        sandboxes.clone(),
        workspaces.clone(),
        AgentService::new(
            store.clone(),
            profiles.clone(),
            Arc::new(agents::TestRegistry),
            agents::agent_capabilities(),
        ),
        profiles,
        Arc::new(PlatformDefaults {
            sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_contract".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(
                WorkspaceProfileId("wkp_contract".into()),
                1,
            ),
            agent_profile: AgentProfileRef::new(AgentProfileId("agp_route".into()), 1),
        }),
    );
    Scenario {
        router: crate::responses::router(state).merge(hierarchy_router(HierarchyState::new(
            sandboxes,
            workspaces,
            HierarchyReadService::new(store),
            Arc::new(instances::MemoryOperationStore::default()),
        ))),
        subject: Some("owner-a"),
        authorization: Authorization::new([Capability::CreateSandbox, Capability::UseOwnSandboxes]),
        symbols: BTreeMap::new(),
    }
}

async fn sandbox() -> Scenario {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    instances::publish_sandbox_profile(
        &profiles,
        "sbp_contract",
        "owner-a",
        ProfileVisibility::Private,
        vec![],
    )
    .await;
    let prerequisite = Arc::new(RecoveringPrerequisite {
        owner: SubjectId::new("owner-a"),
        refuse_next: AtomicBool::new(true),
    });
    Scenario {
        router: hierarchy_router(HierarchyState::new(
            SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                prerequisite,
            ),
            WorkspaceService::new(store.clone(), profiles),
            HierarchyReadService::new(store),
            Arc::new(instances::MemoryOperationStore::default()),
        )),
        subject: Some("owner-a"),
        authorization: Authorization::new([Capability::CreateSandbox, Capability::UseOwnSandboxes]),
        symbols: BTreeMap::new(),
    }
}

async fn launch(name: &str) -> Scenario {
    let starting = name == "credential_launch_start";
    let prerequisite = Arc::new(RecoveringPrerequisite {
        owner: SubjectId::new(VISITOR),
        refuse_next: AtomicBool::new(!starting),
    });
    let app = TestApp::with_credential_prerequisite(prerequisite.clone()).await;
    app.broker
        .set_requirement(ServiceRequirementState::Satisfied);
    let trigger = create_trigger(&app, "always_new", "credential-seed-trigger").await;
    let published = publish_trigger(
        &app,
        trigger["id"].as_str().unwrap(),
        "credential-seed-publish",
    )
    .await;
    let mut symbols = BTreeMap::from([
        ("trigger_id".into(), trigger["id"].clone()),
        ("public_id".into(), published["public_id"].clone()),
    ]);
    if starting {
        let (status, draft) = json_response(
            &app.router,
            visitor_request(
                Method::POST,
                &format!(
                    "/api/v1/triggers/public/{}/launches",
                    published["public_id"].as_str().unwrap()
                ),
                Some(json!({"start_if_ready": false})),
                Some("credential-seed-draft"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{draft}");
        symbols.insert("launch_id".into(), draft["id"].clone());
        prerequisite.refuse_next.store(true, Ordering::SeqCst);
    }
    Scenario {
        router: app.router,
        subject: Some(VISITOR),
        authorization: visitor_authority(),
        symbols,
    }
}
