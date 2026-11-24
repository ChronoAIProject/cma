use super::*;
use layer1_public::rest;
use layer2_core::profiles::ProfileCapacityBounds;
use layer2_core::sandbox::agent_io::{AgentCommandService, AgentCommandWorker};
use layer2_core::sandbox::task_experience::TaskExperiencePolicy;
use layer2_core::sandbox::{
    agent_artifacts::AgentArtifactService, agent_filesystem::AgentFileService,
    answer_deliverables::AnswerDeliverableService,
};

pub(super) async fn router(
    harness: &Harness<MongoSandboxStore>,
    database: &database::Database,
    credentials: Arc<crate::sandbox::credential::ManagedSandboxCredentials>,
    provider: &str,
) -> (Router, Arc<AgentCommandWorker>) {
    let policy = Arc::new(
        serde_json::from_value::<TaskExperiencePolicy>(json!({
            "mode":"accounts", "accounts":["owner"], "capabilities":[
                "task_home_admission", "suggested_followup_admission", "desktop_enable",
                "browser_takeover", "replay_presentation"
            ]
        }))
        .unwrap(),
    );
    router_with_policy(harness, database, credentials, provider, policy).await
}

pub(super) async fn router_with_policy(
    harness: &Harness<MongoSandboxStore>,
    database: &database::Database,
    credentials: Arc<crate::sandbox::credential::ManagedSandboxCredentials>,
    provider: &str,
    policy: Arc<TaskExperiencePolicy>,
) -> (Router, Arc<AgentCommandWorker>) {
    let store = harness.store.clone();
    let profiles = database.stores.sandbox_profiles.clone();
    let authority = Arc::new(authority::Authority);
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let commands = AgentCommandService::new(store.clone(), access.clone(), authority.clone())
        .with_task_experience(policy.clone());
    let services = AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        store.clone(),
        profiles.clone(),
        harness.registry.clone(),
        Arc::new(crate::projections::AdapterSandboxResources::new(
            harness.adapter.clone(),
        )),
        RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Steer,
            RuntimeCapability::Interrupt,
            RuntimeCapability::Usage,
            RuntimeCapability::Resources,
        ]),
    )
    .with_resource_access(access.clone())
    .with_message_feedback(database.stores.message_feedback.clone())
    .with_suggestions(store.clone())
    .with_commands(commands.clone());
    let prerequisite = Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready);
    let sandboxes = SandboxService::new(
        store.clone(),
        profiles.clone(),
        SandboxPolicy::default(),
        prerequisite,
    );
    let workspaces = WorkspaceService::new(store.clone(), profiles.clone());
    let seed = store
        .get_sandbox(&harness.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let defaults = PlatformDefaults {
        sandbox_profile: SandboxProfileRef::new(seed.profile.id, seed.profile.revision),
        workspace_profile: WorkspaceProfileRef::new(
            harness.workspace.profile.id.clone(),
            harness.workspace.profile.revision,
        ),
        agent_profile: AgentProfileRef::new(agent_profile(vec![]).id, 1),
    };
    let context = AgentContextService::new(
        sandboxes.clone(),
        workspaces.clone(),
        services.agents(),
        profiles.clone(),
        Arc::new(defaults),
    );
    let worker = AgentCommandWorker::new(
        store.clone(),
        Arc::new(crate::commands::ResourceCommandExecution {
            source_guard: None,
            authority: authority.clone(),
            access: access.clone(),
            store: store.clone(),
            operations: services.operations(),
            context: context.clone(),
        }),
        "ticket19-command-worker".into(),
    )
    .unwrap();
    let lifecycle = InlineAgentLifecycle::new(
        sandboxes.clone(),
        workspaces.clone(),
        services.agents(),
        store.clone(),
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        profiles,
        ImageManagerResolver::new("http://127.0.0.1:1".into(), false).unwrap(),
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        desktop::capacity(),
        None,
        Arc::new([] as [String; 0]),
        database.stores.agent_lifecycle.clone(),
        None,
        Some(
            layer2_core::sandbox::desktop_creation::DesktopCreationService::new(
                harness.adapter.clone(),
                desktop::image(),
                Some(desktop::image()),
            )
            .unwrap(),
        ),
    )
    .with_task_experience(policy.clone());
    let services = services
        .with_lifecycle(lifecycle.clone())
        .with_lifecycle_operations(lifecycle);
    let files =
        AgentFileService::new(access.clone(), harness.adapter.clone()).with_task_experience(policy);
    let artifacts = AgentArtifactService::new(store.clone(), access.clone());
    let deliveries = AnswerDeliverableService::new(services.reads(), artifacts.clone())
        .with_files(files.clone());
    let hierarchy = rest::HierarchyState::new(
        sandboxes,
        workspaces,
        layer2_core::sandbox::HierarchyReadService::new(store.clone())
            .with_resource_access(access.clone()),
        database.stores.operations.clone(),
    );
    let flags = crate::config::FeatureFlags {
        v2_ui_transport: true,
        ..Default::default()
    };
    let router = services
        .router(database.stores.operations.clone())
        .merge(catalog::router(provider))
        .merge(rest::agent_models_router(
            services.reads(),
            crate::agent_models::ChronoAgentModels::new(
                &std::env::var("CMA_INTEGRATED_MODEL_ORIGIN").unwrap(),
                credentials,
            )
            .unwrap(),
            Arc::new(crate::projections::AdapterAgentModelSupport(
                harness.adapter.clone(),
            )),
        ))
        .merge(rest::agent_source_router(
            services.reads(),
            layer2_core::trigger_domain::TriggerService::new(
                database.stores.trigger_definitions.clone(),
                database.stores.trigger_definitions.clone(),
                layer2_core::UserDirectory::new(database.stores.users.clone()),
            ),
        ))
        .merge(services.conversation_router(context, harness.updates.clone(), "http://127.0.0.1"))
        .merge(services.agent_events_router(flags, harness.updates.clone()))
        .merge(layer1_public::responses::v2::router(
            layer1_public::responses::v2::ResponsesV2State::new(
                layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
                commands,
                services.reads(),
            ),
        ))
        .merge(rest::hierarchy_router(hierarchy))
        .merge(rest::resource_access_router(access.clone()))
        .merge(rest::agent_provenance_router(
            layer2_core::sandbox::AgentProvenanceService::new(store.clone(), access.clone()),
        ))
        .merge(rest::agent_access_requests_router(
            crate::agent_access::AgentAccessService::new(
                store.clone(),
                authority,
                Arc::new(SandboxPolicy::default()),
            ),
        ))
        .merge(rest::agent_filesystem_router(files))
        .merge(rest::agent_artifacts_router(artifacts))
        .merge(rest::agent_attachments_router(
            layer2_core::sandbox::agent_attachments::AgentAttachmentService::new(
                access.clone(),
                harness.adapter.clone(),
            ),
        ))
        .merge(rest::answer_deliverables_router(deliveries))
        .merge(rest::agent_channel_usage_router(
            layer2_core::sandbox::channel_usage::AgentChannelUsageService::new(store, access),
        ))
        .route(
            "/openapi.json",
            axum::routing::get(|| async { axum::Json(rest::openapi::document()) }),
        )
        .layer(middleware::from_fn(authority::authenticate));
    (router, worker)
}
