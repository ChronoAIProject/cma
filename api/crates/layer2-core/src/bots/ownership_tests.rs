use super::*;
use crate::{profiles::*, resource_access::*, Authorization, Caller, Capability, SubjectId};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

fn context(subject: &str, organization: Option<&str>) -> CallerContext {
    let mut context = CallerContext {
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::ManageOwnBots, Capability::ManageOwnProfiles]),
        ),
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "https://identity.invalid".into(),
            subject: SubjectId::new(subject),
            credential_id: "current-login".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    };
    if let Some(organization) = organization {
        context.memberships.push(
            OrganizationMembership::new(
                context.credential.clone(),
                SubjectId::new(organization),
                OrganizationRole::Admin,
                SystemTime::now(),
                SystemTime::now() + Duration::from_secs(300),
            )
            .unwrap(),
        );
    }
    context
}
fn authority(subject: &str, organization: Option<&str>) -> BotOwnerAuthority {
    let owner = match organization {
        Some(org) => ResourceOwner::Organization {
            subject: SubjectId::new(org),
        },
        None => ResourceOwner::Person {
            subject: SubjectId::new(subject),
        },
    };
    BotOwnerAuthority::new(context(subject, organization), owner, None).unwrap()
}
fn service() -> (Arc<BotService>, Arc<InMemoryBotStore>) {
    let store = Arc::new(InMemoryBotStore::default());
    (
        BotService::new(
            store.clone(),
            Arc::new(super::tests::Profiles),
            "registered".into(),
        ),
        store,
    )
}

#[tokio::test]
async fn organization_ownership_retains_actual_actor_and_custody_and_replays() {
    let (service, _) = service();
    let mut context = context("human", Some("org"));
    context.actor = ResourceActor::Api {
        subject: SubjectId::new("human"),
        delegation_id: Some("grant-a".into()),
    };
    let owner = ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    };
    let authority = BotOwnerAuthority::new(
        context,
        owner.clone(),
        Some(BotDelegation {
            authorization_grant_id: "grant-a".into(),
            credential_incarnation: "login-a".into(),
        }),
    )
    .unwrap();
    let request = super::tests::input();
    let bot = service
        .create_for_owner(&authority, "setup-a", request.clone())
        .await
        .unwrap();
    assert_eq!(bot.owner, SubjectId::new("org"));
    assert_eq!(bot.resource_owner(), owner);
    assert_eq!(bot.credential_subject(), &SubjectId::new("human"));
    assert_eq!(bot.credential_incarnation(), Some("login-a"));
    assert_eq!(bot.authorization_grant_id(), Some("grant-a"));
    assert_eq!(
        service
            .create_for_owner(&authority, "setup-a", request)
            .await
            .unwrap(),
        bot
    );
    assert_eq!(
        service.get(&super::tests::caller("org"), &bot.bot_id).await,
        Err(BotError::NotFound)
    );
    assert_eq!(
        service
            .get_for_owner(&authority, &bot.bot_id)
            .await
            .unwrap(),
        bot
    );
    let snapshot = crate::event_workers::projection::snapshot_for_owner(
        &crate::event_workers::projection::WorkerProjection {
            name: &bot.request.name,
            owner: &bot.owner,
            provider: &bot.provider,
            profiles: &bot.profiles,
        },
        &bot.resource_owner(),
    )
    .unwrap();
    assert_eq!(snapshot["metadata"]["owner"]["kind"], "organization");
    let mut altered = bot.clone();
    altered.ownership.as_mut().unwrap().credential_incarnation = Some("later-login".into());
    altered.revision = bot.revision + 1;
    assert!(altered.validate_replacement(&bot).is_err());
}

#[test]
fn stale_membership_wrong_credential_non_admin_and_other_person_are_forbidden() {
    let org = ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    };
    let mut stale = context("human", Some("org"));
    stale.memberships[0].expires_at = SystemTime::now() - Duration::from_secs(1);
    let mut wrong_credential = context("human", Some("org"));
    wrong_credential.memberships[0].credential.credential_id = "different-login".into();
    let mut member = context("human", Some("org"));
    member.memberships[0].role = OrganizationRole::Member;
    let mut binding = context("human", Some("org"));
    binding.credential_source = CredentialSource::Binding;
    for value in [stale, wrong_credential, member, binding] {
        assert!(matches!(
            BotOwnerAuthority::new(value, org.clone(), None),
            Err(BotError::Forbidden)
        ));
    }
    assert!(matches!(
        BotOwnerAuthority::new(
            context("human", None),
            ResourceOwner::Person {
                subject: SubjectId::new("other")
            },
            None
        ),
        Err(BotError::Forbidden)
    ));
}

#[tokio::test(start_paused = true)]
async fn expired_authority_after_profile_io_never_inserts_a_bot() {
    struct DelayedProfiles;
    #[async_trait::async_trait]
    impl BotProfileResolver for DelayedProfiles {
        async fn resolve(
            &self,
            owner: &SubjectId,
            request: &CreateBot,
        ) -> Result<BotProfiles, BotError> {
            tokio::time::advance(Duration::from_secs(61)).await;
            super::tests::Profiles.resolve(owner, request).await
        }
    }
    let store = Arc::new(InMemoryBotStore::default());
    let service = BotService::new(
        store.clone(),
        Arc::new(DelayedProfiles),
        "registered".into(),
    );
    assert_eq!(
        service
            .create_for_owner(
                &authority("human", Some("org")),
                "setup-a",
                super::tests::input()
            )
            .await,
        Err(BotError::Forbidden)
    );
    assert!(store.pending(None, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn personal_legacy_serialization_is_unchanged_and_owner_kinds_do_not_collide() {
    let (service, _) = service();
    let request = super::tests::input();
    let legacy = service
        .create(&super::tests::caller("org"), "same-key", request.clone())
        .await
        .unwrap();
    assert!(serde_json::to_value(&legacy)
        .unwrap()
        .get("ownership")
        .is_none());
    let personal = service
        .create_for_owner(&authority("org", None), "same-key", request.clone())
        .await
        .unwrap();
    let organization = service
        .create_for_owner(&authority("human", Some("org")), "same-key", request)
        .await
        .unwrap();
    assert_ne!(legacy.bot_id, personal.bot_id);
    assert_ne!(personal.bot_id, organization.bot_id);
    assert_eq!(
        service
            .list(&super::tests::caller("org"), None, 10)
            .await
            .unwrap()
            .len(),
        2
    );
}

fn profiles(store: Arc<InMemoryProfileStore>) -> Arc<PrivateProfileCreationService> {
    PrivateProfileCreationService::new(
        store,
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        Arc::new([] as [String; 0]),
    )
}
fn agent_content() -> AgentProfileContent {
    AgentProfileContent {
        isolated_agent: false,
        model: Some("test".into()),
        thinking_effort: ThinkingEffort::Medium,
        permission_level: PermissionLevel::Guarded,
        skills: vec![],
        system_prompt: "Be helpful".into(),
    }
}

#[tokio::test]
async fn private_custom_profiles_pin_revisions_and_enforce_child_ownership() {
    let store = Arc::new(InMemoryProfileStore::default());
    let service = profiles(store.clone());
    let owner = authority("human", Some("org"));
    let agent = service
        .create_agent(&owner, "setup", "Agent".into(), "".into(), agent_content())
        .await
        .unwrap();
    assert_eq!(
        service
            .create_agent(&owner, "setup", "Agent".into(), "".into(), agent_content())
            .await
            .unwrap(),
        agent
    );
    let mut changed = agent_content();
    changed.system_prompt = "Different".into();
    assert!(matches!(
        service
            .create_agent(&owner, "setup", "Agent".into(), "".into(), changed)
            .await,
        Err(ProfileCreationError::Conflict)
    ));
    let workspace = WorkspaceProfileContent {
        location: WorkspaceLocation::Directory {
            name: "work".into(),
        },
        name: "work".into(),
        setup_commands: vec![],
        agent_profiles: vec![AgentProfileRef::new(agent.profile_id.clone(), 1)],
    };
    assert!(matches!(
        service
            .create_workspace(
                &authority("other", None),
                "setup",
                "Workspace".into(),
                "".into(),
                workspace.clone()
            )
            .await,
        Err(ProfileCreationError::Store(
            ProfileStoreError::ReferenceUnavailable
        ))
    ));
    let workspace = service
        .create_workspace(&owner, "setup", "Workspace".into(), "".into(), workspace)
        .await
        .unwrap();
    let sandbox = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1000,
        memory_mib: 2048,
        nyxid_services: vec![],
        agent_provider: AgentProvider::default(),
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 1,
        max_agents_per_workspace: 1,
        workspace_profiles: vec![WorkspaceProfileRef::new(workspace.profile_id.clone(), 1)],
        software_install_commands: vec![],
        environment_variables: vec![],
    };
    let sandbox = service
        .create_sandbox(&owner, "setup", "Sandbox".into(), "".into(), sandbox)
        .await
        .unwrap();
    let agent_row = store
        .agent_profile(&agent.profile_id)
        .await
        .unwrap()
        .unwrap();
    let workspace_row = store
        .workspace_profile(&workspace.profile_id)
        .await
        .unwrap()
        .unwrap();
    let sandbox_row = store
        .sandbox_profile(&sandbox.profile_id)
        .await
        .unwrap()
        .unwrap();
    for (owner, visibility) in [
        (agent_row.owner, agent_row.visibility),
        (workspace_row.owner, workspace_row.visibility),
        (sandbox_row.owner, sandbox_row.visibility),
    ] {
        assert_eq!(owner, SubjectId::new("org"));
        assert_eq!(visibility, ProfileVisibility::Private);
    }
    assert_eq!(agent.revision, 1);
    assert_eq!(workspace.revision, 1);
    assert_eq!(sandbox.revision, 1);
}

#[tokio::test]
async fn profile_creation_resumes_an_inserted_private_draft_and_preserves_keep_alive() {
    let store = Arc::new(InMemoryProfileStore::default());
    let service = profiles(store.clone());
    let owner = authority("human", Some("org"));
    let key = "interrupted-setup";
    let digest = content_digest(&("private-owned-profile-v1", owner.owner(), "agp", key)).unwrap();
    let id = crate::AgentProfileId(format!("agp_{}", &digest[..32]));
    let now = crate::now_rfc3339();
    store
        .create_agent_profile(AgentProfile {
            internal: false,
            id: id.clone(),
            owner: owner.owner_subject().clone(),
            name: "Agent".into(),
            description: "".into(),
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: agent_content(),
            draft_version: 1,
            created_at: now.clone(),
            updated_at: now,
            retired_at: None,
        })
        .await
        .unwrap();
    let revision = service
        .create_agent(&owner, key, "Agent".into(), "".into(), agent_content())
        .await
        .unwrap();
    assert_eq!(revision.profile_id, id);
    assert_eq!(revision.revision, 1);
    let input = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 1000,
        memory_mib: 2048,
        nyxid_services: vec![],
        agent_provider: AgentProvider::default(),
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 1,
        max_agents_per_workspace: 1,
        workspace_profiles: vec![],
        software_install_commands: vec![],
        environment_variables: vec![],
    };
    let availability = SandboxProfileAvailability {
        always_keep_alive: true,
        ..Default::default()
    };
    let revision = service
        .create_sandbox_configured(
            &owner,
            key,
            PrivateSandboxProfileInput {
                name: "Sandbox".into(),
                description: "".into(),
                content: input,
                availability,
            },
        )
        .await
        .unwrap();
    let row = store
        .sandbox_profile(&revision.profile_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.availability, availability);
    assert_eq!(row.visibility, ProfileVisibility::Private);
}
