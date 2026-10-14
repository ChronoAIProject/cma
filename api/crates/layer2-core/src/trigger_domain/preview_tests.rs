use crate::ids::{AgentProfileId, SandboxProfileId, WorkspaceProfileId};
use crate::profiles::{
    AgentProfileContent, AgentProfileRef, AgentProfileRevision, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileRef, SandboxProfileRevision, WorkspaceLocation,
    WorkspaceProfileContent, WorkspaceProfileRef,
};

use super::*;
use crate::trigger_domain::{
    AgentReusePolicy, Presentation, ReusePolicy, SandboxReusePolicy, TriggerConsent, TriggerTarget,
    WorkspaceReusePolicy,
};

const PUBLISHED: &str = "2026-09-05T00:00:00Z";
const NESTED_FIELD: &str = "target.sandbox_profile.workspace_profiles";
const FULL_FIRST_MESSAGE: &str = concat!(
    "This deliberately long first message is safe to summarize anonymously, but its complete ",
    "contents must stay hidden until launch. The distinctive final clause proves that projection ",
    "returns only the bounded prefix."
);

struct Fixture {
    revision: TriggerRevision,
    profiles: ResolvedTriggerProfiles,
}

fn workspace(
    id: &str,
    name: &str,
    location: WorkspaceLocation,
    commands: &[&str],
) -> WorkspaceProfileRevision {
    let content = WorkspaceProfileContent {
        location,
        name: name.into(),
        setup_commands: commands.iter().map(|command| (*command).into()).collect(),
        agent_profiles: Vec::new(),
    };
    WorkspaceProfileRevision {
        profile_id: WorkspaceProfileId(id.into()),
        revision: 1,
        digest: content_digest(&content).unwrap(),
        content,
        published_at: PUBLISHED.into(),
    }
}

fn directory_workspace(id: &str, name: &str, commands: &[&str]) -> WorkspaceProfileRevision {
    workspace(
        id,
        name,
        WorkspaceLocation::Directory { name: name.into() },
        commands,
    )
}

fn fixture(
    nested: Vec<WorkspaceProfileRevision>,
    target: WorkspaceProfileRevision,
    sandbox_commands: &[&str],
    environment_variables: Vec<EnvironmentVariable>,
) -> Fixture {
    let sandbox_content = SandboxProfileContent {
        runtime_image: None,
        cpu_millis: 2_000,
        memory_mib: 4_096,
        nyxid_services: Vec::new(),
        agent_provider: AgentProvider::default(),
        model_access: ModelAccess::ChronoaiManaged,
        max_workspaces: 8,
        max_agents_per_workspace: 8,
        workspace_profiles: nested
            .iter()
            .map(|revision| {
                WorkspaceProfileRef::new(revision.profile_id.clone(), revision.revision)
            })
            .collect(),
        software_install_commands: sandbox_commands
            .iter()
            .map(|command| (*command).into())
            .collect(),
        environment_variables,
    };
    let sandbox = SandboxProfileRevision {
        profile_id: SandboxProfileId("sbp_preview".into()),
        revision: 3,
        digest: content_digest(&sandbox_content).unwrap(),
        content: sandbox_content,
        published_at: PUBLISHED.into(),
    };
    let agent_content = AgentProfileContent {
        isolated_agent: false,
        model: Some("gpt-5".into()),
        thinking_effort: ThinkingEffort::High,
        permission_level: PermissionLevel::Guarded,
        skills: Vec::new(),
        system_prompt: "hidden agent instructions".into(),
    };
    let agent = AgentProfileRevision {
        profile_id: AgentProfileId("agp_preview".into()),
        revision: 2,
        digest: content_digest(&agent_content).unwrap(),
        content: agent_content,
        published_at: PUBLISHED.into(),
    };
    let target_reference = TriggerTarget {
        sandbox_profile: SandboxProfileRef::new(sandbox.profile_id.clone(), sandbox.revision),
        workspace_profile: WorkspaceProfileRef::new(target.profile_id.clone(), target.revision),
        agent_profile: AgentProfileRef::new(agent.profile_id.clone(), agent.revision),
        inline: None,
    };
    let revision = TriggerRevision::new(
        1,
        TriggerContent {
            presentation: Presentation {
                title: "Preview".into(),
                description: String::new(),
                button_label: ButtonLabel::OpenInCma,
                theme: Theme::Auto,
                signup_hint: None,
            },
            target: target_reference,
            reuse: ReusePolicy {
                sandbox: SandboxReusePolicy::AlwaysNew,
                workspace: WorkspaceReusePolicy::AlwaysNew,
                agent: AgentReusePolicy::AlwaysNew,
            },
            first_message: FULL_FIRST_MESSAGE.into(),
            quick_messages: Vec::new(),
            consent: TriggerConsent {
                always_review: false,
            },
        },
        PUBLISHED.into(),
    )
    .unwrap();
    Fixture {
        revision,
        profiles: ResolvedTriggerProfiles {
            sandbox,
            sandbox_workspaces: nested,
            workspace: target,
            agent,
        },
    }
}

fn preview(fixture: &Fixture) -> TriggerPreview {
    TriggerPreview::project(
        TriggerAvailability::Published,
        &fixture.revision,
        "Publisher".into(),
        &fixture.profiles,
    )
    .unwrap()
}

fn assert_mismatch(
    revision: &TriggerRevision,
    profiles: &ResolvedTriggerProfiles,
    field: &'static str,
) {
    let error = TriggerPreview::project(
        TriggerAvailability::Published,
        revision,
        "Publisher".into(),
        profiles,
    )
    .unwrap_err();
    assert_eq!(
        error,
        TriggerError::new("trigger_reference_mismatch", field)
    );
}

#[test]
fn disclosures_follow_sandbox_nested_and_ad_hoc_execution_order() {
    let nested_a = directory_workspace("wkp_nested_a", "nested-a", &["prepare a"]);
    let nested_b = workspace(
        "wkp_nested_b",
        "",
        WorkspaceLocation::GitRepository {
            url: "https://github.com/chronoai/research-kit.git".into(),
            reference: Some("main".into()),
        },
        &["prepare b", "verify b"],
    );
    let target = directory_workspace("wkp_target", "target", &["prepare target"]);
    let environment = vec![EnvironmentVariable {
        name: "TOOLCHAIN_CACHE".into(),
        value: "/opt/toolchain".into(),
    }];
    let fixture = fixture(
        vec![nested_a.clone(), nested_b.clone()],
        target.clone(),
        &["install compiler"],
        environment.clone(),
    );

    let preview = preview(&fixture);
    assert_eq!(
        preview.trust.disclosures,
        vec![
            TriggerTrustPreviewDisclosure::Sandbox {
                commands_digest: fixture.profiles.sandbox.digest.clone(),
                commands: vec!["install compiler".into()],
                environment_variables: environment,
            },
            TriggerTrustPreviewDisclosure::Workspace {
                name: "nested-a".into(),
                commands_digest: nested_a.digest,
                commands: vec!["prepare a".into()],
            },
            TriggerTrustPreviewDisclosure::Workspace {
                name: "research-kit".into(),
                commands_digest: nested_b.digest,
                commands: vec!["prepare b".into(), "verify b".into()],
            },
            TriggerTrustPreviewDisclosure::Workspace {
                name: "target".into(),
                commands_digest: target.digest,
                commands: vec!["prepare target".into()],
            },
        ]
    );

    let json = serde_json::to_string(&preview).unwrap();
    for hidden in [
        "sbp_preview",
        "wkp_nested_a",
        "wkp_nested_b",
        "wkp_target",
        "agp_preview",
        "https://github.com/chronoai/research-kit.git",
        "hidden agent instructions",
        FULL_FIRST_MESSAGE,
    ] {
        assert!(!json.contains(hidden), "preview exposed {hidden}");
    }
}

#[test]
fn nested_target_is_disclosed_once_at_its_sandbox_position() {
    let nested_a = directory_workspace("wkp_nested_a", "first", &["prepare first"]);
    let target = directory_workspace("wkp_target", "target", &["prepare target"]);
    let nested_b = directory_workspace("wkp_nested_b", "last", &["prepare last"]);
    let fixture = fixture(
        vec![nested_a, target.clone(), nested_b],
        target,
        &[],
        Vec::new(),
    );

    let disclosures = preview(&fixture).trust.disclosures;
    let names: Vec<_> = disclosures
        .iter()
        .map(|disclosure| match disclosure {
            TriggerTrustPreviewDisclosure::Workspace { name, .. } => name.as_str(),
            TriggerTrustPreviewDisclosure::Sandbox { .. } => panic!("unexpected Sandbox"),
        })
        .collect();
    assert_eq!(names, ["first", "target", "last"]);
    assert_eq!(names.iter().filter(|name| **name == "target").count(), 1);
}

#[test]
fn safe_scopes_are_omitted_without_reordering_mutating_scopes() {
    let safe_nested = directory_workspace("wkp_safe", "safe", &[]);
    let mutating_nested = directory_workspace("wkp_mutating", "mutating", &["configure"]);
    let safe_target = directory_workspace("wkp_target", "target", &[]);
    let environment = vec![EnvironmentVariable {
        name: "CACHE_ROOT".into(),
        value: "/cache".into(),
    }];
    let fixture = fixture(
        vec![safe_nested, mutating_nested.clone()],
        safe_target,
        &[],
        environment.clone(),
    );

    assert_eq!(
        preview(&fixture).trust.disclosures,
        vec![
            TriggerTrustPreviewDisclosure::Sandbox {
                commands_digest: fixture.profiles.sandbox.digest.clone(),
                commands: Vec::new(),
                environment_variables: environment,
            },
            TriggerTrustPreviewDisclosure::Workspace {
                name: "mutating".into(),
                commands_digest: mutating_nested.digest,
                commands: vec!["configure".into()],
            },
        ]
    );
}

#[test]
fn projection_rejects_missing_reordered_or_corrupt_resolved_revisions() {
    let nested_a = directory_workspace("wkp_nested_a", "nested-a", &["prepare a"]);
    let nested_b = directory_workspace("wkp_nested_b", "nested-b", &["prepare b"]);
    let target = directory_workspace("wkp_target", "target", &["prepare target"]);
    let fixture = fixture(
        vec![nested_a, nested_b],
        target,
        &["install compiler"],
        Vec::new(),
    );

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox_workspaces.pop();
    assert_mismatch(&fixture.revision, &profiles, NESTED_FIELD);

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox_workspaces.swap(0, 1);
    assert_mismatch(&fixture.revision, &profiles, NESTED_FIELD);

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox_workspaces[0].profile_id = WorkspaceProfileId("wkp_wrong".into());
    assert_mismatch(&fixture.revision, &profiles, NESTED_FIELD);

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox_workspaces[0].revision += 1;
    assert_mismatch(&fixture.revision, &profiles, NESTED_FIELD);

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox_workspaces[0]
        .content
        .setup_commands
        .push("undisclosed mutation".into());
    assert_mismatch(&fixture.revision, &profiles, NESTED_FIELD);

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox.digest = "corrupt".into();
    assert_mismatch(&fixture.revision, &profiles, "target.sandbox_profile");

    let mut profiles = fixture.profiles.clone();
    profiles.workspace.digest = "corrupt".into();
    assert_mismatch(&fixture.revision, &profiles, "target.workspace_profile");

    let mut profiles = fixture.profiles.clone();
    profiles.agent.digest = "corrupt".into();
    assert_mismatch(&fixture.revision, &profiles, "target.agent_profile");
}

#[test]
fn invalid_workspace_names_fail_closed_at_their_origin() {
    let nested = directory_workspace("wkp_nested", "nested", &["prepare nested"]);
    let target = directory_workspace("wkp_target", "target", &["prepare target"]);
    let fixture = fixture(vec![nested], target, &[], Vec::new());

    let mut profiles = fixture.profiles.clone();
    profiles.sandbox_workspaces[0].content.name.clear();
    profiles.sandbox_workspaces[0].digest =
        content_digest(&profiles.sandbox_workspaces[0].content).unwrap();
    assert_mismatch(&fixture.revision, &profiles, NESTED_FIELD);

    let mut profiles = fixture.profiles.clone();
    profiles.workspace.content.name.clear();
    profiles.workspace.digest = content_digest(&profiles.workspace.content).unwrap();
    assert_mismatch(&fixture.revision, &profiles, "target.workspace_profile");
}
