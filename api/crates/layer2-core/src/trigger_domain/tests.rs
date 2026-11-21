use std::sync::Arc;

use crate::ids::{
    AgentProfileId, SandboxProfileId, ServiceId, SkillId, SubjectId, WorkspaceProfileId,
};
use crate::profiles::{
    AgentProfile, AgentProfileContent, AgentProfileRef, AgentProfileStore, AgentProvider,
    EnvironmentVariable, InMemoryProfileStore, MarketplaceLocator, MarketplaceStore, ModelAccess,
    PermissionLevel, ProfileCapacityBounds, ProfileVisibility, SandboxProfile,
    SandboxProfileContent, SandboxProfileRef, SandboxProfileStore, SkillRef, SkillSource,
    ThinkingEffort, WorkspaceLocation, WorkspaceProfile, WorkspaceProfileContent,
    WorkspaceProfileRef, WorkspaceProfileStore,
};

use super::*;

const CREATED: &str = "2026-09-05T00:00:00Z";
const PUBLISHED: &str = "2026-09-05T01:00:00Z";
const UPDATED: &str = "2026-09-05T02:00:00Z";
const RETIRED: &str = "2026-09-05T03:00:00Z";

#[derive(Clone, Copy)]
pub(super) struct Visibilities {
    sandbox: ProfileVisibility,
    workspace: ProfileVisibility,
    agent: ProfileVisibility,
}

impl Default for Visibilities {
    fn default() -> Self {
        Self {
            sandbox: ProfileVisibility::Public,
            workspace: ProfileVisibility::Public,
            agent: ProfileVisibility::Public,
        }
    }
}

pub(super) struct SeededProfiles {
    pub(super) profiles: Arc<InMemoryProfileStore>,
    pub(super) target: TriggerTarget,
    pub(super) owner: SubjectId,
}

pub(super) async fn seed_profiles(
    visibility: Visibilities,
    workspace_in_sandbox: bool,
) -> SeededProfiles {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let owner = SubjectId("publisher".into());
    let agent_id = AgentProfileId("agp_agent".into());
    let workspace_id = WorkspaceProfileId("wkp_workspace".into());
    let sandbox_id = SandboxProfileId("sbp_sandbox".into());

    let agent = AgentProfile {
        internal: false,
        id: agent_id.clone(),
        owner: owner.clone(),
        name: "Research Agent".into(),
        description: "Checks a proof.".into(),
        visibility: visibility.agent,
        forked_from: None,
        uses_count: 0,
        draft: AgentProfileContent {
            isolated_agent: false,
            model: Some("gpt-5".into()),
            thinking_effort: ThinkingEffort::High,
            permission_level: PermissionLevel::Guarded,
            skills: vec![SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("lean"),
                version: Some("1".into()),
            }],
            system_prompt: "Check the requested proof.".into(),
        },
        draft_version: 1,
        created_at: CREATED.into(),
        updated_at: CREATED.into(),
        retired_at: None,
    };
    profiles
        .create_agent_profile(agent)
        .await
        .expect("agent profile");
    profiles
        .publish_agent_profile(&agent_id, 1, PUBLISHED.into())
        .await
        .expect("agent revision");

    let workspace = WorkspaceProfile {
        internal: false,
        id: workspace_id.clone(),
        owner: owner.clone(),
        name: "Proof Workspace".into(),
        description: "A small proof workspace.".into(),
        visibility: visibility.workspace,
        forked_from: None,
        uses_count: 0,
        draft: WorkspaceProfileContent {
            location: WorkspaceLocation::Directory {
                name: "proofs".into(),
            },
            name: "proofs".into(),
            setup_commands: vec!["lake update".into()],
            agent_profiles: vec![AgentProfileRef::new(agent_id.clone(), 1)],
        },
        draft_version: 1,
        created_at: CREATED.into(),
        updated_at: CREATED.into(),
        retired_at: None,
    };
    profiles
        .create_workspace_profile(workspace)
        .await
        .expect("workspace profile");
    profiles
        .publish_workspace_profile(&workspace_id, 1, PUBLISHED.into())
        .await
        .expect("workspace revision");

    let sandbox = SandboxProfile {
        internal: false,
        availability: Default::default(),
        id: sandbox_id.clone(),
        owner: owner.clone(),
        name: "Lean Sandbox".into(),
        description: "Lean and Git.".into(),
        visibility: visibility.sandbox,
        forked_from: None,
        uses_count: 0,
        draft: SandboxProfileContent {
            runtime_image: None,
            cpu_millis: 2_000,
            memory_mib: 4_096,
            nyxid_services: vec![ServiceId::new("github")],
            agent_provider: AgentProvider::default(),
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 4,
            max_agents_per_workspace: 4,
            workspace_profiles: workspace_in_sandbox
                .then(|| WorkspaceProfileRef::new(workspace_id.clone(), 1))
                .into_iter()
                .collect(),
            software_install_commands: vec!["install-lean".into()],
            environment_variables: vec![EnvironmentVariable {
                name: "LEAN_PATH".into(),
                value: "/home/cma/lean".into(),
            }],
        },
        draft_version: 1,
        created_at: CREATED.into(),
        updated_at: CREATED.into(),
        retired_at: None,
    };
    profiles
        .create_sandbox_profile(sandbox)
        .await
        .expect("sandbox profile");
    profiles
        .publish_sandbox_profile(
            &sandbox_id,
            1,
            PUBLISHED.into(),
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8_000,
                min_memory_mib: 128,
                max_memory_mib: 16_384,
            },
            &[],
        )
        .await
        .expect("sandbox revision");

    SeededProfiles {
        profiles,
        target: TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(sandbox_id, 1),
            workspace_profile: WorkspaceProfileRef::new(workspace_id, 1),
            agent_profile: AgentProfileRef::new(agent_id, 1),
            inline: None,
        },
        owner,
    }
}

pub(super) fn content(target: TriggerTarget) -> TriggerContent {
    TriggerContent {
        presentation: Presentation {
            title: "Check this theorem".into(),
            description: "Open a prepared Lean workspace.".into(),
            button_label: ButtonLabel::RunWithCma,
            theme: Theme::Auto,
            signup_hint: Some("Use your personal account.".into()),
        },
        target,
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::Ask,
        },
        first_message: "Check theorem example.".into(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: true,
        },
    }
}

pub(super) fn trigger(owner: SubjectId, target: TriggerTarget) -> Trigger {
    Trigger::new(
        TriggerId::generate(),
        owner,
        content(target),
        CREATED.into(),
    )
    .expect("trigger")
}

pub(super) fn trigger_store(seed: &SeededProfiles) -> InMemoryTriggerStore {
    let resolver: Arc<dyn TriggerProfileResolver> = seed.profiles.clone();
    InMemoryTriggerStore::new(resolver)
}

#[test]
fn generated_ids_are_opaque_unique_and_prefixed() {
    let trigger_a = TriggerId::generate();
    let trigger_b = TriggerId::generate();
    assert_ne!(trigger_a, trigger_b);
    assert!(trigger_a.as_str().starts_with("trg_"));
    assert!(PublicTriggerId::generate().as_str().starts_with("pub_"));
    assert!(PlacementId::generate().as_str().starts_with("plc_"));
}

#[tokio::test]
async fn content_validation_covers_every_bounded_or_required_value() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let valid = content(seed.target.clone());
    valid.validate().expect("valid content");

    let mut value = valid.clone();
    value.presentation.title = " ".into();
    assert_eq!(value.validate().unwrap_err().code, "invalid_title");
    let mut value = valid.clone();
    value.presentation.title = "x".repeat(121);
    assert_eq!(value.validate().unwrap_err().code, "invalid_title");
    let mut value = valid.clone();
    value.presentation.description = "x".repeat(2_001);
    assert_eq!(value.validate().unwrap_err().code, "description_too_long");
    let mut value = valid.clone();
    value.presentation.signup_hint = Some("x".repeat(201));
    assert_eq!(value.validate().unwrap_err().code, "signup_hint_too_long");
    let mut value = valid.clone();
    value.first_message = "x".repeat(MAX_FIRST_MESSAGE_BYTES + 1);
    assert_eq!(value.validate().unwrap_err().code, "first_message_too_long");
    let mut value = valid;
    value.target.agent_profile.revision = 0;
    assert_eq!(
        value.validate().unwrap_err().code,
        "invalid_profile_reference"
    );
    let invalid = Trigger::new(
        TriggerId("wrong".into()),
        seed.owner,
        content(seed.target),
        CREATED.into(),
    );
    assert_eq!(invalid.unwrap_err().code, "invalid_trigger_id");
}

#[test]
fn placement_validation_normalizes_fragments_and_rejects_unsafe_urls() {
    let placement = Placement::new(
        "https://example.com/readme?tab=one#launch".into(),
        "README".into(),
    )
    .expect("placement");
    assert_eq!(placement.source_url, "https://example.com/readme?tab=one");
    assert!(Placement::new("http://example.com".into(), String::new()).is_err());
    assert!(Placement::new("https://user@example.com".into(), String::new()).is_err());
    assert!(Placement::new("https://example.com".into(), "x".repeat(81)).is_err());
    let too_long = format!("https://example.com/{}", "x".repeat(500));
    assert_eq!(
        Placement::new(too_long, String::new()).unwrap_err().code,
        "placement_url_too_long"
    );
}

#[tokio::test]
async fn aggregate_state_machine_keeps_one_public_id_and_immutable_revisions() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let mut aggregate = trigger(seed.owner, seed.target);
    let placement = Placement::new("https://example.com/trigger".into(), "Trigger".into()).unwrap();
    let placement_id = placement.id.clone();
    aggregate.add_placement(placement).unwrap();
    assert_eq!(aggregate.availability, TriggerAvailability::Draft);
    assert!(aggregate.public_id.is_none());

    let first = aggregate.publish(PUBLISHED.into()).expect("first publish");
    let public_id = aggregate.public_id.clone().expect("public id");
    aggregate
        .replace_draft(1, aggregate.draft.clone(), UPDATED.into())
        .expect("new draft");
    let second = aggregate.publish(UPDATED.into()).expect("second publish");
    assert_eq!(aggregate.public_id, Some(public_id));
    assert_eq!((first.revision, second.revision), (1, 2));
    assert_eq!(aggregate.revisions[0], first);

    aggregate.record_launch().expect("launch");
    aggregate.retire(RETIRED.into()).expect("retire");
    aggregate
        .record_agent_ready()
        .expect("in-flight Agent ready");
    assert_eq!(aggregate.retire(RETIRED.into()), Ok(()));
    assert_eq!(
        aggregate.record_launch().unwrap_err().code,
        "trigger_not_published"
    );
    assert_eq!(
        aggregate
            .replace_draft(2, aggregate.draft.clone(), UPDATED.into())
            .unwrap_err()
            .code,
        "trigger_retired"
    );
    assert_eq!(
        aggregate
            .retire_placement(&placement_id, RETIRED.into())
            .unwrap_err()
            .code,
        "trigger_retired"
    );
    assert!(aggregate.placements[0].retired_at.is_none());
    aggregate.validate().expect("valid terminal aggregate");
}

#[tokio::test]
async fn placement_state_machine_enforces_live_limit_and_reopens_capacity() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let mut aggregate = trigger(seed.owner, seed.target);
    for index in 0..MAX_LIVE_PLACEMENTS {
        aggregate
            .add_placement(
                Placement::new(
                    format!("https://example.com/{index}"),
                    format!("Placement {index}"),
                )
                .expect("placement"),
            )
            .expect("within limit");
    }
    let extra = Placement::new("https://example.com/extra".into(), String::new()).unwrap();
    assert_eq!(
        aggregate.add_placement(extra).unwrap_err().code,
        "too_many_placements"
    );
    let retired = aggregate.placements[0].id.clone();
    aggregate
        .retire_placement(&retired, RETIRED.into())
        .expect("retire placement");
    aggregate
        .add_placement(Placement::new("https://example.com/extra".into(), String::new()).unwrap())
        .expect("retired placement releases capacity");
}

#[tokio::test]
async fn trigger_content_digest_has_a_canonical_golden_value() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    assert_eq!(
        trigger_content_digest(&content(seed.target)).unwrap(),
        "4844e7c345df0c6c177aa8179afcfbc52dd2bf0f87b01b6c798fac38625ff23b"
    );
}

#[tokio::test]
async fn publishing_resolves_exact_public_revisions_and_keeps_latest_link() {
    let seed = seed_profiles(Visibilities::default(), false).await;
    let store = trigger_store(&seed);
    let aggregate = trigger(seed.owner.clone(), seed.target.clone());
    let id = aggregate.id.clone();
    store.create_trigger(aggregate).await.unwrap();

    let first = store
        .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
        .await
        .expect("public profiles publish even when Workspace is added at launch");
    let saved = store
        .save_draft(
            &id,
            &seed.owner,
            1,
            content(seed.target.clone()),
            UPDATED.into(),
        )
        .await
        .unwrap();
    assert_eq!(saved.draft_version, 2);
    let conflict = store
        .save_draft(
            &id,
            &seed.owner,
            1,
            content(seed.target.clone()),
            UPDATED.into(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        conflict,
        TriggerStoreError::Conflict { actual: 2, .. }
    ));
    let second = store
        .publish_trigger(&id, &seed.owner, 2, UPDATED.into())
        .await
        .unwrap();
    assert_eq!(first.public_id, second.public_id);
    assert_eq!(second.revision.revision, 2);
    assert_eq!(first.revision.digest, second.revision.digest);
    assert_eq!(
        store
            .trigger_revision(&id, &first.revision.digest)
            .await
            .unwrap(),
        Some(first.revision.clone())
    );
    assert!(store
        .trigger_revision(&TriggerId("trg_other".into()), &first.revision.digest)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .trigger_by_public_id(&first.public_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        second.revision
    );
}

#[tokio::test]
async fn immutable_profile_reader_ignores_current_retirement_state() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    seed.profiles
        .retire_agent_profile(&seed.target.agent_profile.id, &seed.owner, RETIRED.into())
        .await
        .unwrap();
    let reader: Arc<dyn TriggerProfileReader> = seed.profiles;

    let profiles = reader.read_trigger_profiles(&seed.target).await.unwrap();

    assert_eq!(profiles.sandbox.profile_id, seed.target.sandbox_profile.id);
    assert_eq!(profiles.sandbox_workspaces.len(), 1);
    assert_eq!(
        profiles.workspace.profile_id,
        seed.target.workspace_profile.id
    );
    assert_eq!(profiles.agent.profile_id, seed.target.agent_profile.id);
}

#[tokio::test]
async fn publication_refuses_private_missing_and_retired_profile_references() {
    for (visibility, field) in [
        (
            Visibilities {
                sandbox: ProfileVisibility::Private,
                ..Visibilities::default()
            },
            "target.sandbox_profile",
        ),
        (
            Visibilities {
                workspace: ProfileVisibility::Private,
                ..Visibilities::default()
            },
            "target.workspace_profile",
        ),
        (
            Visibilities {
                agent: ProfileVisibility::Private,
                ..Visibilities::default()
            },
            "target.agent_profile",
        ),
    ] {
        let seed = seed_profiles(visibility, false).await;
        let store = trigger_store(&seed);
        let aggregate = trigger(seed.owner.clone(), seed.target);
        let id = aggregate.id.clone();
        store.create_trigger(aggregate).await.unwrap();
        let error = store
            .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
            .await
            .unwrap_err();
        assert_eq!(error.code(), "trigger_reference_not_public");
        assert_eq!(
            match error {
                TriggerStoreError::Reference(reference) => reference.field(),
                other => panic!("unexpected error: {other:?}"),
            },
            field
        );
    }

    let mut seed = seed_profiles(Visibilities::default(), true).await;
    seed.target.agent_profile.revision = 99;
    let store = trigger_store(&seed);
    let aggregate = trigger(seed.owner.clone(), seed.target);
    let id = aggregate.id.clone();
    store.create_trigger(aggregate).await.unwrap();
    assert_eq!(
        store
            .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
            .await
            .unwrap_err()
            .code(),
        "trigger_reference_unavailable"
    );

    let seed = seed_profiles(Visibilities::default(), true).await;
    seed.profiles
        .retire_agent_profile(&seed.target.agent_profile.id, &seed.owner, RETIRED.into())
        .await
        .unwrap();
    let store = trigger_store(&seed);
    let aggregate = trigger(seed.owner.clone(), seed.target);
    let id = aggregate.id.clone();
    store.create_trigger(aggregate).await.unwrap();
    assert_eq!(
        store
            .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
            .await
            .unwrap_err()
            .code(),
        "trigger_reference_unavailable"
    );
}

#[tokio::test]
async fn publication_observes_visibility_changed_after_draft_creation() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let store = trigger_store(&seed);
    let aggregate = trigger(seed.owner.clone(), seed.target.clone());
    let id = aggregate.id.clone();
    store.create_trigger(aggregate).await.unwrap();

    // This is the profile-mutation-wins side of the publication race contract.
    seed.profiles
        .set_profile_visibility(
            &seed.owner,
            &MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
            ProfileVisibility::Private,
            UPDATED.into(),
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8_000,
                min_memory_mib: 128,
                max_memory_mib: 16_384,
            },
            &[],
        )
        .await
        .unwrap();

    let error = store
        .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "trigger_reference_not_public");
}

#[tokio::test]
async fn preview_reads_published_revisions_after_profile_becomes_private() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let store = trigger_store(&seed);
    let aggregate = trigger(seed.owner.clone(), seed.target.clone());
    let id = aggregate.id.clone();
    store.create_trigger(aggregate).await.unwrap();
    let public = store
        .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
        .await
        .unwrap();

    seed.profiles
        .set_profile_visibility(
            &seed.owner,
            &MarketplaceLocator::Sandbox(seed.target.sandbox_profile.id.clone()),
            ProfileVisibility::Private,
            UPDATED.into(),
            ProfileCapacityBounds {
                min_cpu_millis: 100,
                max_cpu_millis: 8_000,
                min_memory_mib: 128,
                max_memory_mib: 16_384,
            },
            &[],
        )
        .await
        .unwrap();

    let preview = store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .expect("exact immutable revisions remain readable")
        .expect("published Trigger remains available");
    assert_eq!(preview.title, "Check this theorem");
}

#[tokio::test]
async fn anonymous_preview_hides_profile_ids_and_full_first_message() {
    let seed = seed_profiles(Visibilities::default(), false).await;
    let store = trigger_store(&seed);
    let mut aggregate = trigger(seed.owner.clone(), seed.target.clone());
    let full_message = "proof ".repeat(40);
    aggregate.draft.first_message = full_message.clone();
    let id = aggregate.id.clone();
    store.create_trigger(aggregate).await.unwrap();
    let public = store
        .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
        .await
        .unwrap();
    let preview = store
        .preview_by_public_id(&public.public_id, "Ada Lovelace".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preview.first_message.chars().count(), 140);
    assert_eq!(
        preview.signup_hint.as_deref(),
        Some("Use your personal account.")
    );
    assert_eq!(preview.what_happens.len(), 3);
    let sandbox_revision = seed
        .profiles
        .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let workspace_revision = seed
        .profiles
        .workspace_profile_revision(&seed.target.workspace_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        preview.trust.disclosures,
        vec![
            TriggerTrustPreviewDisclosure::Sandbox {
                commands_digest: sandbox_revision.digest,
                commands: vec!["install-lean".into()],
                environment_variables: sandbox_revision.content.environment_variables,
            },
            TriggerTrustPreviewDisclosure::Workspace {
                name: "proofs".into(),
                commands_digest: workspace_revision.digest,
                commands: vec!["lake update".into()],
            },
        ]
    );
    assert_eq!(preview.technical.provider, AgentProvider::default());
    assert_eq!(preview.technical.model.as_deref(), Some("gpt-5"));
    assert_eq!(preview.technical.effort, ThinkingEffort::High);
    assert_eq!(preview.technical.size.memory_mib, 4_096);
    let json = serde_json::to_string(&preview).unwrap();
    assert!(!json.contains(seed.target.sandbox_profile.id.as_str()));
    assert!(!json.contains(seed.target.workspace_profile.id.as_str()));
    assert!(!json.contains(seed.target.agent_profile.id.as_str()));
    assert!(!json.contains(&full_message));
    assert!(!json.contains("Check the requested proof."));
    assert!(json.contains("\"signup_hint\":\"Use your personal account.\""));
    assert!(!json.contains("target"));
}

#[tokio::test]
async fn placement_crud_counters_and_owner_list_are_complete() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let store = trigger_store(&seed);
    let first = trigger(seed.owner.clone(), seed.target.clone());
    let first_id = first.id.clone();
    store.create_trigger(first).await.unwrap();
    let other = Trigger::new(
        TriggerId("trg_other".into()),
        SubjectId("other".into()),
        content(seed.target.clone()),
        CREATED.into(),
    )
    .unwrap();
    store.create_trigger(other).await.unwrap();

    let placement = store
        .create_placement(
            &first_id,
            &seed.owner,
            "https://example.com/page#button".into(),
            "Page".into(),
        )
        .await
        .unwrap();
    assert_eq!(placement.source_url, "https://example.com/page");
    let updated = store
        .update_placement(
            &first_id,
            &seed.owner,
            &placement.id,
            PlacementUpdate {
                source_url: "https://example.com/other".into(),
                label: "Other".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.label, "Other");
    assert_eq!(
        store
            .placement(&first_id, &seed.owner, &placement.id)
            .await
            .unwrap(),
        Some(updated)
    );
    store
        .retire_placement(&first_id, &seed.owner, &placement.id, RETIRED.into())
        .await
        .unwrap();
    assert!(store
        .list_placements(&first_id, &seed.owner, false)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .list_placements(&first_id, &seed.owner, true)
            .await
            .unwrap()
            .len(),
        1
    );

    assert_eq!(
        store
            .record_agent_ready(&first_id)
            .await
            .unwrap_err()
            .code(),
        "agent_ready_without_launch"
    );
    store
        .publish_trigger(&first_id, &seed.owner, 1, PUBLISHED.into())
        .await
        .unwrap();
    assert_eq!(
        store.record_launch(&first_id).await.unwrap().launches_total,
        1
    );
    assert_eq!(
        store
            .record_agent_ready(&first_id)
            .await
            .unwrap()
            .agents_ready_total,
        1
    );
    let page = store
        .list_own_triggers(&seed.owner, &TriggerPageRequest::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
}

#[tokio::test]
async fn published_profile_retirement_guard_returns_every_trigger_id() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let store = trigger_store(&seed);
    let mut ids = Vec::new();
    for suffix in ["b", "a"] {
        let aggregate = Trigger::new(
            TriggerId(format!("trg_{suffix}")),
            seed.owner.clone(),
            content(seed.target.clone()),
            CREATED.into(),
        )
        .unwrap();
        ids.push(aggregate.id.clone());
        store.create_trigger(aggregate).await.unwrap();
        store
            .publish_trigger(ids.last().unwrap(), &seed.owner, 1, PUBLISHED.into())
            .await
            .unwrap();
    }
    ids.sort();
    for error in [
        store
            .guard_sandbox_profile_retirement(&seed.target.sandbox_profile.id)
            .await
            .unwrap_err(),
        store
            .guard_workspace_profile_retirement(&seed.target.workspace_profile.id)
            .await
            .unwrap_err(),
        store
            .guard_agent_profile_retirement(&seed.target.agent_profile.id)
            .await
            .unwrap_err(),
    ] {
        assert_eq!(error.code(), "profile_referenced_by_published_trigger");
        assert_eq!(
            error,
            TriggerStoreError::ProfileReferenced {
                trigger_ids: ids.clone()
            }
        );
    }
    for id in &ids {
        store
            .retire_trigger(id, &seed.owner, RETIRED.into())
            .await
            .unwrap();
    }
    store
        .guard_agent_profile_retirement(&seed.target.agent_profile.id)
        .await
        .expect("retired Triggers do not block profile retirement");
    let public = store
        .trigger(&ids[0], &seed.owner)
        .await
        .unwrap()
        .unwrap()
        .public_id
        .unwrap();
    assert_eq!(
        store
            .trigger_by_public_id(&public)
            .await
            .unwrap()
            .unwrap()
            .availability,
        TriggerAvailability::Retired
    );
    assert_eq!(
        store
            .preview_by_public_id(&public, "Publisher".into())
            .await
            .unwrap()
            .unwrap()
            .availability,
        TriggerAvailability::Retired
    );
}

#[tokio::test]
async fn own_trigger_listing_is_bounded_and_cursor_paged() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let store = trigger_store(&seed);
    for suffix in ["c", "a", "b"] {
        store
            .create_trigger(
                Trigger::new(
                    TriggerId(format!("trg_{suffix}")),
                    seed.owner.clone(),
                    content(seed.target.clone()),
                    CREATED.into(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let first = store
        .list_own_triggers(
            &seed.owner,
            &TriggerPageRequest {
                limit: 2,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["trg_a", "trg_b"]
    );
    let second = store
        .list_own_triggers(
            &seed.owner,
            &TriggerPageRequest {
                limit: 2,
                cursor: first.next,
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items[0].id.as_str(), "trg_c");
    assert!(second.next.is_none());
    assert_eq!(
        store
            .list_own_triggers(
                &seed.owner,
                &TriggerPageRequest {
                    limit: 0,
                    cursor: None,
                },
            )
            .await
            .unwrap_err()
            .code(),
        "invalid_trigger_page"
    );
}
