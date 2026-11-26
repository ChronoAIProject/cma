use std::collections::BTreeMap;

use super::*;

fn options() -> RuntimeOptions {
    RuntimeOptions {
        model: Some("gpt-test".into()),
        reasoning_effort: ReasoningEffort::High,
        permission: PermissionLevel::Autonomous,
        own_credential: false,
        isolated_agent: false,
        working_directory: "/home/cma/repo/.worktrees/agent-a".into(),
        home_directory: "/home/cma/.cma/agents/agent-a/home".into(),
        slot: AgentSlot::new("agent-a"),
    }
}

fn skill() -> AgentSkill {
    AgentSkill {
        id: "review".into(),
        name: None,
        version: Some("7".into()),
        archive_sha256: "ab".repeat(32),
        source_url: "https://skills.example/review.tar.gz".into(),
    }
}

#[test]
fn legacy_skill_payload_without_name_round_trips_unchanged() {
    let payload = serde_json::to_value(skill()).unwrap();
    assert!(payload.get("name").is_none());
    let decoded: AgentSkill = serde_json::from_value(payload.clone()).unwrap();
    assert_eq!(decoded.name, None);
    assert_eq!(serde_json::to_value(decoded).unwrap(), payload);
}

#[test]
fn restart_runtime_identity_preserves_the_open_request_digest() {
    let mut request = AgentOpenRequest::new("agent:agent-a:open:2:v1", options(), None);
    let original = serde_json::to_value(&request).unwrap();
    assert!(original.get("restart_runtime").is_none());
    request.restart_runtime = Some(crate::AgentRestartRuntime {
        runtime: crate::RuntimeStatusRequest {
            sandbox_id: "sandbox".into(),
            generation: 7,
            home_identity: "a".repeat(43),
        },
        boot_id: "b".repeat(32),
    });
    assert!(request.has_valid_sha256());
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(encoded["request_sha256"], original["request_sha256"]);
    assert_eq!(
        serde_json::from_value::<AgentOpenRequest>(encoded).unwrap(),
        request
    );
}

#[test]
fn agent_digest_encodings_are_stable() {
    let home = AgentHomeRequest::new(
        "agent:agent-a:home:create:v1",
        options(),
        "Inspect carefully.",
        vec![skill()],
    );
    let open = AgentOpenRequest::new(
        "agent:agent-a:open:v1",
        options(),
        Some("conversation-7".into()),
    );
    let close = AgentCloseRequest::new("agent:agent-a:close:v1", AgentSlot::new("agent-a"));
    let remove =
        AgentHomeRemoveRequest::new("agent:agent-a:home:remove:v1", AgentSlot::new("agent-a"));
    let plan = AgentHomePlan {
        operation_id: home.operation_id.clone(),
        request_sha256: home.request_sha256.clone(),
        agent_id: AgentSlot::new("agent-a"),
        files: vec![HomeFile {
            path: "AGENTS.md".into(),
            bytes: b"Inspect carefully.".to_vec(),
        }],
        symlinks: vec![HomeSymlink {
            path: "auth.json".into(),
            target: "/shared/auth.json".into(),
        }],
        skills: vec![skill()],
    };
    let launch = AgentLaunch {
        operation_id: open.operation_id.clone(),
        request_sha256: open.request_sha256.clone(),
        agent_id: AgentSlot::new("agent-a"),
        cwd: options().working_directory,
        home: options().home_directory,
        program: "agent-bridge".into(),
        args: vec!["--stdio".into()],
        env: BTreeMap::from([("AGENT_MODE".into(), "test".into())]),
    };

    assert_eq!(
        [
            home.request_sha256.as_str(),
            open.request_sha256.as_str(),
            close.request_sha256.as_str(),
            remove.request_sha256.as_str(),
            plan.payload_sha256().as_str(),
            launch.payload_sha256().as_str(),
        ],
        [
            "7594a829ef8dbe072630db33fc5d98da68f16740d80b5ac511ef1d35610f4584",
            "8c9cd7c60738544eb96a2fafbc8bb7c78f7a4f5a889a3a94b51540b1a6b5e1ca",
            "47a9a9cf05141b83ed00fbc42f4f4694138d386e62582a1ad6a0539410e25635",
            "3e5ed595a6404ed06f8475c7c4978b5cf056d4d728523979e7795f6fb1649e17",
            "d6e850ddbbf529add7ff102f38fd6711b99d0da2bb14fb7c1d9a6bcd44c293cc",
            "72f4fa8cb924668a96ceff7455166f0cffea17cd312eed4a05d3aee1c6dd50d8",
        ]
    );
}

#[test]
fn agent_request_digests_cover_every_semantic_execution_field() {
    let home = AgentHomeRequest::new(
        "agent:agent-a:home:create:v1",
        options(),
        "Inspect carefully.",
        vec![skill()],
    );
    let expected = home.request_sha256.clone();
    let mut changed = Vec::new();
    macro_rules! changed_home {
        ($field:expr) => {{
            let mut request = home.clone();
            $field(&mut request);
            changed.push(request.computed_sha256());
        }};
    }
    changed_home!(|request: &mut AgentHomeRequest| request.options.model = None);
    changed_home!(
        |request: &mut AgentHomeRequest| request.options.reasoning_effort = ReasoningEffort::Low
    );
    changed_home!(
        |request: &mut AgentHomeRequest| request.options.permission = PermissionLevel::Guarded
    );
    changed_home!(|request: &mut AgentHomeRequest| request.options.own_credential = true);
    changed_home!(|request: &mut AgentHomeRequest| request.options.isolated_agent = true);
    changed_home!(|request: &mut AgentHomeRequest| request
        .options
        .working_directory
        .push_str("-other"));
    changed_home!(|request: &mut AgentHomeRequest| request
        .options
        .home_directory
        .push_str("-other"));
    changed_home!(|request: &mut AgentHomeRequest| request.options.slot = AgentSlot::new("agent-b"));
    changed_home!(|request: &mut AgentHomeRequest| request.system_prompt.push('!'));
    changed_home!(|request: &mut AgentHomeRequest| request.skills[0].id.push_str("-other"));
    changed_home!(
        |request: &mut AgentHomeRequest| request.skills[0].name = Some("named-skill".into())
    );
    changed_home!(|request: &mut AgentHomeRequest| request.skills[0].version = None);
    changed_home!(|request: &mut AgentHomeRequest| request.skills[0]
        .archive_sha256
        .replace_range(..2, "cd"));
    changed_home!(|request: &mut AgentHomeRequest| request.skills.push(skill()));
    assert!(changed.iter().all(|digest| digest != &expected));
    let mut refreshed_locator = home.clone();
    refreshed_locator.skills[0].source_url.push_str("?rotated");
    assert_eq!(refreshed_locator.computed_sha256(), expected);

    let open = AgentOpenRequest::new(
        "agent:agent-a:open:v1",
        options(),
        Some("conversation-7".into()),
    );
    let mut changed_open = open.clone();
    changed_open.resume_conversation = None;
    assert_ne!(changed_open.computed_sha256(), open.request_sha256);

    let close = AgentCloseRequest::new("agent:agent-a:close:v1", AgentSlot::new("agent-a"));
    let changed_close = AgentCloseRequest::new("agent:agent-a:close:v1", AgentSlot::new("agent-b"));
    assert_ne!(changed_close.request_sha256, close.request_sha256);
    let remove =
        AgentHomeRemoveRequest::new("agent:agent-a:home:remove:v1", AgentSlot::new("agent-a"));
    let changed_remove =
        AgentHomeRemoveRequest::new("agent:agent-a:home:remove:v1", AgentSlot::new("agent-b"));
    assert_ne!(changed_remove.request_sha256, remove.request_sha256);
}

#[test]
fn rendered_payload_digests_cover_materialization_and_launch_fields() {
    let plan = AgentHomePlan {
        operation_id: "agent:agent-a:home:create:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("agent-a"),
        files: vec![HomeFile {
            path: "AGENTS.md".into(),
            bytes: b"prompt".to_vec(),
        }],
        symlinks: vec![HomeSymlink {
            path: "auth.json".into(),
            target: "/shared/auth.json".into(),
        }],
        skills: vec![skill()],
    };
    let expected = plan.payload_sha256();
    let mut plans = Vec::new();
    macro_rules! changed_plan {
        ($field:expr) => {{
            let mut changed = plan.clone();
            $field(&mut changed);
            plans.push(changed.payload_sha256());
        }};
    }
    changed_plan!(|plan: &mut AgentHomePlan| plan.agent_id = AgentSlot::new("agent-b"));
    changed_plan!(|plan: &mut AgentHomePlan| plan.files[0].path.push_str("-other"));
    changed_plan!(|plan: &mut AgentHomePlan| plan.files[0].bytes.push(0));
    changed_plan!(|plan: &mut AgentHomePlan| plan.symlinks[0].path.push_str("-other"));
    changed_plan!(|plan: &mut AgentHomePlan| plan.symlinks[0].target.push_str("-other"));
    changed_plan!(|plan: &mut AgentHomePlan| plan.skills[0].name = Some("named-skill".into()));
    assert!(plans.iter().all(|digest| digest != &expected));
    let mut refreshed_locator = plan.clone();
    refreshed_locator.skills[0].source_url.push_str("?rotated");
    assert_eq!(refreshed_locator.payload_sha256(), expected);

    let launch = AgentLaunch {
        operation_id: "agent:agent-a:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent-a"),
        cwd: "/worktree".into(),
        home: "/home".into(),
        program: "bridge".into(),
        args: vec!["--stdio".into()],
        env: BTreeMap::from([("MODE".into(), "test".into())]),
    };
    let expected = launch.payload_sha256();
    let mut launches = Vec::new();
    macro_rules! changed_launch {
        ($field:expr) => {{
            let mut changed = launch.clone();
            $field(&mut changed);
            launches.push(changed.payload_sha256());
        }};
    }
    changed_launch!(|launch: &mut AgentLaunch| launch.agent_id = AgentSlot::new("agent-b"));
    changed_launch!(|launch: &mut AgentLaunch| launch.cwd.push_str("-other"));
    changed_launch!(|launch: &mut AgentLaunch| launch.home.push_str("-other"));
    changed_launch!(|launch: &mut AgentLaunch| launch.program.push_str("-other"));
    changed_launch!(|launch: &mut AgentLaunch| launch.args.push("--other".into()));
    changed_launch!(|launch: &mut AgentLaunch| {
        launch.env.insert("OTHER".into(), "value".into());
    });
    assert!(launches.iter().all(|digest| digest != &expected));
}
