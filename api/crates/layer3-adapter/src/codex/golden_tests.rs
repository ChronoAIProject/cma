use super::*;
use crate::{
    AgentHomeRequest, AgentOpenRequest, AgentSkill, AgentSlot, PermissionLevel, ReasoningEffort,
    RuntimeOptions,
};

#[test]
fn adapter_exposes_browser_and_runtime_recovery_through_the_shared_contract() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let adapter: &dyn SandboxAdapter = &adapter;
    assert!(adapter.browser().is_some());
    assert!(adapter.runtime_recovery().is_some());
}

fn options(
    own_credential: bool,
    permission: PermissionLevel,
    effort: ReasoningEffort,
) -> RuntimeOptions {
    RuntimeOptions {
        model: Some("gpt-golden".into()),
        reasoning_effort: effort,
        permission,
        own_credential,
        isolated_agent: false,
        working_directory: "/home/cma/repo/.worktrees/wt-7".into(),
        home_directory: "/home/cma/.cma/agents/agent-7/home".into(),
        slot: AgentSlot("agent-7".into()),
    }
}

fn golden(own_credential: bool, permission: PermissionLevel, effort: ReasoningEffort) -> String {
    let (approval, sandbox) = match permission {
        PermissionLevel::Guarded => ("on-request", "read-only"),
        PermissionLevel::Autonomous => ("never", "danger-full-access"),
    };
    let prefix = format!(
        "model = \"gpt-golden\"\n\
         model_reasoning_effort = \"{}\"\n\
         approval_policy = \"{approval}\"\n\
         sandbox_mode = \"{sandbox}\"\n",
        effort.as_str()
    );
    if own_credential {
        prefix
    } else {
        format!(
            "model = \"gpt-golden\"\n\
             model_provider = \"cma\"\n\
             model_reasoning_effort = \"{}\"\n\
             approval_policy = \"{approval}\"\n\
             sandbox_mode = \"{sandbox}\"\n\
             \n\
             [model_providers.cma]\n\
             name = \"CMA Loopback\"\n\
             base_url = \"http://127.0.0.1:9091\"\n\
             env_key = \"CMA_MODEL_PROXY_PLACEHOLDER\"\n\
             wire_api = \"responses\"\n\
             request_max_retries = 2\n\
             stream_max_retries = 1\n\
             stream_idle_timeout_ms = 300000\n",
            effort.as_str()
        )
    }
}

#[test]
fn config_goldens_cover_both_access_modes_permissions_and_all_efforts() {
    let efforts = [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
    ];
    for own_credential in [false, true] {
        for permission in [PermissionLevel::Guarded, PermissionLevel::Autonomous] {
            for effort in efforts {
                let rendered = render_config(
                    &options(own_credential, permission, effort),
                    &CodexConfig::default(),
                )
                .expect("valid config renders");
                assert_eq!(
                    rendered,
                    golden(own_credential, permission, effort),
                    "access={own_credential} permission={permission:?} effort={effort:?}"
                );
                assert_eq!(rendered.contains("[model_providers.cma]"), !own_credential);
            }
        }
    }
}

#[test]
fn managed_stream_limits_bound_a_silent_model_call() {
    use super::config::{
        MANAGED_REQUEST_MAX_RETRIES, MANAGED_STREAM_IDLE_TIMEOUT_MS, MANAGED_STREAM_MAX_RETRIES,
    };
    // Codex re-runs a sampling request after an idle or failed stream, and
    // each run retries its HTTP request first. The two limits multiply.
    let attempts = MANAGED_STREAM_MAX_RETRIES + 1;
    let silent_ms = attempts * MANAGED_STREAM_IDLE_TIMEOUT_MS;
    assert!(
        silent_ms <= 10 * 60 * 1000,
        "silent stream bound {silent_ms} ms"
    );
    // A gateway timeout answers after about 100 s on each request.
    let gateway_timeouts = attempts * (MANAGED_REQUEST_MAX_RETRIES + 1);
    assert!(gateway_timeouts * 100_000 <= 10 * 60 * 1000);
    // Codex's implicit defaults: six idle runs of 300 s, thirty timeouts.
    assert!(silent_ms < 6 * 300_000 && gateway_timeouts < 6 * 5);

    let own = render_config(
        &options(true, PermissionLevel::Autonomous, ReasoningEffort::High),
        &CodexConfig::default(),
    )
    .unwrap();
    assert!(!own.contains("max_retries") && !own.contains("stream_idle_timeout_ms"));
}

#[test]
fn agent_home_contains_exact_profile_files_shared_auth_and_skills() {
    let prompt = "Line one\n\nDo exactly this.\n";
    let request = AgentHomeRequest::new(
        "agent:agent-7:home:create:v1",
        options(false, PermissionLevel::Guarded, ReasoningEffort::High),
        prompt,
        vec![AgentSkill {
            id: "proof-review".into(),
            name: None,
            version: Some("v3".into()),
            archive_sha256: "a".repeat(64),
            source_url: "https://ornn.example/proof-review.tgz".into(),
        }],
    );
    let plan = render_agent_home(&request, &CodexConfig::default()).expect("home renders");

    assert_eq!(plan.agent_id, request.options.slot);
    assert_eq!(
        plan.files
            .iter()
            .find(|file| file.path == "AGENTS.md")
            .expect("instructions file")
            .bytes,
        prompt.as_bytes()
    );
    assert!(plan.files.iter().any(|file| file.path == "config.toml"));
    assert!(plan.symlinks.iter().any(|link| {
        link.path == "auth.json" && link.target == "/home/cma/.cma/provider/codex/auth.json"
    }));
    for baseline in ["nyxid", "ornn-agent-manual-cli"] {
        assert!(plan
            .symlinks
            .iter()
            .any(|link| link.path == format!("skills/{baseline}")));
    }
    assert_eq!(plan.skills, request.skills);
}

#[test]
fn selected_baseline_skills_use_pinned_archives_without_duplicate_fallback_links() {
    for selected in [vec!["nyxid"], vec!["nyxid", "ornn-agent-manual-cli"]] {
        let request = AgentHomeRequest::new(
            "agent:agent-7:home:create:v1",
            options(false, PermissionLevel::Autonomous, ReasoningEffort::XHigh),
            "Use the selected skill revisions.",
            selected
                .iter()
                .map(|id| AgentSkill {
                    id: (*id).into(),
                    name: None,
                    version: Some("2.0".into()),
                    archive_sha256: "a".repeat(64),
                    source_url: format!("https://ornn.example/{id}.tgz"),
                })
                .collect(),
        );
        let plan = render_agent_home(&request, &CodexConfig::default()).unwrap();
        assert_eq!(plan.skills, request.skills);
        for id in ["nyxid", "ornn-agent-manual-cli"] {
            assert_eq!(
                plan.symlinks
                    .iter()
                    .any(|link| link.path == format!("skills/{id}")),
                !selected.contains(&id)
            );
        }
        let mut duplicate = request;
        duplicate.skills.push(duplicate.skills[0].clone());
        assert!(render_agent_home(&duplicate, &CodexConfig::default()).is_err());
    }
}

#[test]
fn registry_skill_names_replace_fallbacks_even_when_ids_are_opaque() {
    let skills = ["nyxid", "ornn-agent-manual-cli"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| AgentSkill {
            id: format!("registry-id-{index}"),
            name: Some(name.into()),
            version: Some("3.0".into()),
            archive_sha256: "c".repeat(64),
            source_url: format!("https://ornn.example/registry-id-{index}.zip"),
        })
        .collect();
    let request = AgentHomeRequest::new(
        "agent:agent-7:home:create:v1",
        options(false, PermissionLevel::Autonomous, ReasoningEffort::XHigh),
        "",
        skills,
    );
    let plan = render_agent_home(&request, &CodexConfig::default()).unwrap();
    assert_eq!(plan.symlinks.len(), 1);
    assert_eq!(plan.symlinks[0].path, "auth.json");
    for (installed, selected) in plan.skills.iter().zip(&request.skills) {
        assert_eq!(installed.id, selected.id);
        assert_eq!(installed.version, selected.version);
        assert_eq!(installed.archive_sha256, selected.archive_sha256);
        assert_eq!(installed.source_url, selected.source_url);
        assert_eq!(installed.name, None);
        assert!(serde_json::to_value(installed)
            .unwrap()
            .get("name")
            .is_none());
    }

    let mut duplicate_name = request;
    duplicate_name.skills[1].name = duplicate_name.skills[0].name.clone();
    assert!(render_agent_home(&duplicate_name, &CodexConfig::default()).is_err());
}

#[test]
fn bridge_launch_uses_per_agent_home_worktree_slot_and_acp_resume() {
    let options = options(false, PermissionLevel::Autonomous, ReasoningEffort::XHigh);
    let fresh = render_launch(&AgentOpenRequest::new(
        "agent:agent-7:open:v1",
        options.clone(),
        None,
    ))
    .expect("fresh launch");
    let resumed = render_launch(&AgentOpenRequest::new(
        "agent:agent-7:open:v1",
        options.clone(),
        Some("conversation-old".into()),
    ))
    .expect("resume launch");

    assert_ne!(fresh.request_sha256, resumed.request_sha256);
    assert_eq!(fresh.program, resumed.program);
    assert_eq!(fresh.args, resumed.args, "resume is not a CLI argument");
    assert_eq!(fresh.env, resumed.env);
    assert_eq!(fresh.program, "codex-acp");
    assert!(fresh.args.is_empty());
    assert_eq!(fresh.agent_id, options.slot);
    assert_eq!(fresh.cwd, options.working_directory);
    assert_eq!(fresh.home, options.home_directory);
    assert_eq!(fresh.env.get("CODEX_HOME"), Some(&fresh.home));
}

#[test]
fn checkpoint_exclusions_are_exact_sorted_disposable_alias_roots() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    assert_eq!(
        adapter.checkpoint_exclusions(&[]).unwrap(),
        [".codex/tmp/arg0"]
    );
    assert_eq!(
        adapter
            .checkpoint_exclusions(&[
                AgentSlot::new("agent-z"),
                AgentSlot::new("agent-a"),
                AgentSlot::new("agent-z"),
            ])
            .unwrap(),
        [
            ".cma/agents/agent-a/home/tmp/arg0",
            ".cma/agents/agent-z/home/tmp/arg0",
            ".codex/tmp/arg0"
        ]
    );
    for slot in [
        "",
        ".",
        "..",
        "../outside",
        "agent/home",
        "agent\\home",
        "agent*",
        "agent\n",
    ] {
        assert!(adapter
            .checkpoint_exclusions(&[AgentSlot::new(slot)])
            .is_err());
    }
}

#[test]
fn checkpoint_archive_adds_only_the_new_generated_alias_siblings() {
    let old = super::checkpoint_exclusions(&[AgentSlot::new("agent-a")]).unwrap();
    let mut requested = old.clone();
    requested.push("project/tmp/arg0".into());
    requested.push(".cma/agents/agent-z/home/tmp/arg0-sibling".into());
    assert_eq!(
        super::checkpoint_archive_exclusions(&requested),
        [
            ".cma/agents/agent-a/home/tmp/arg0",
            ".cma/agents/agent-a/home/tmp/arg0-linux",
            ".cma/agents/agent-z/home/tmp/arg0-sibling",
            ".codex/tmp/arg0",
            ".codex/tmp/arg0-linux",
            "project/tmp/arg0",
        ]
    );
    assert_eq!(
        old,
        [".cma/agents/agent-a/home/tmp/arg0", ".codex/tmp/arg0"]
    );
}

#[test]
fn bridge_launch_rejects_non_normalized_or_misplaced_worktree_paths() {
    for cwd in [
        "home/cma/repo/.worktrees/wt-7",
        "/home/cma/repo/.worktrees/../outside",
        "/home/cma/repo/./.worktrees/wt-7",
        "/home/cma/repo/not-worktrees/wt-7",
        "/home/cma/repo/.worktrees/wt-7/nested",
    ] {
        let mut options = options(false, PermissionLevel::Guarded, ReasoningEffort::High);
        options.working_directory = cwd.into();

        assert!(
            render_launch(&AgentOpenRequest::new(
                "agent:agent-7:open:v1",
                options,
                None,
            ))
            .is_err(),
            "accepted invalid worktree cwd {cwd}"
        );
    }
}

#[test]
fn managed_launch_placeholder_does_not_change_the_pinned_sandbox_environment() {
    for own_credential in [false, true] {
        let spec = crate::SandboxSpec {
            sandbox_id: "sandbox-7".into(),
            own_credential,
            ..crate::SandboxSpec::default()
        };
        let deployment = crate::hosted::HostedDeployment::default();
        let launch = render_launch(&AgentOpenRequest::new(
            "agent:agent-7:open:v1",
            options(
                own_credential,
                PermissionLevel::Guarded,
                ReasoningEffort::High,
            ),
            None,
        ))
        .expect("valid launch");
        for generation in [1, 2] {
            let runtime = deployment
                .render_runtime(&spec, generation)
                .expect("valid runtime");
            let name = "CMA_MODEL_PROXY_PLACEHOLDER";
            assert!(!runtime.env.contains_key(name));
            assert!(!runtime.env.contains_key("CHRONO_UPSTREAM_PLACEHOLDER_KEY"));
            assert_eq!(launch.env.contains_key(name), !own_credential);
            for (name, value) in &launch.env {
                if name == "HOME"
                    || name == "PATH"
                    || name.starts_with("CHRONO_")
                    || name.starts_with("NYXID_")
                {
                    assert_eq!(runtime.env.get(name), Some(value));
                }
            }
        }
        let mut overridden = spec;
        overridden.environment.insert(
            "CHRONO_UPSTREAM_PLACEHOLDER_KEY".into(),
            "owner-supplied-value".into(),
        );
        assert!(deployment.render(&overridden).is_err());
    }
}

#[test]
fn isolated_agent_uses_a_trusted_outer_boundary_without_shared_auth() {
    let mut choices = options(false, PermissionLevel::Autonomous, ReasoningEffort::High);
    choices.isolated_agent = true;
    let home = AgentHomeRequest::new("isolated-home", choices.clone(), "private prompt", vec![]);
    let plan = render_agent_home(&home, &CodexConfig::default()).unwrap();
    assert!(!plan.symlinks.iter().any(|link| link.path == "auth.json"));
    assert!(plan.symlinks.iter().any(|link| link.path == "skills/nyxid"));
    let launch = render_launch(&AgentOpenRequest::new(
        "isolated-open",
        choices.clone(),
        None,
    ))
    .unwrap();
    assert_eq!(launch.program, "/usr/bin/python3");
    assert_eq!(&launch.args[..2], ["-I", "-c"]);
    assert_eq!(launch.args[2], include_str!("isolate_agent.py"));
    assert_eq!(launch.args[3], "outer");
    assert_eq!(launch.args[4], choices.home_directory);
    assert_eq!(launch.args[5], choices.working_directory);

    choices.own_credential = true;
    assert!(render_launch(&AgentOpenRequest::new(
        "invalid-open",
        choices.clone(),
        None
    ))
    .is_err());
    assert!(render_agent_home(
        &AgentHomeRequest::new("invalid-home", choices, "", vec![]),
        &CodexConfig::default(),
    )
    .is_err());
}

#[test]
fn isolated_agent_relay_and_namespace_regressions() {
    let result = std::process::Command::new("python3")
        .args(["-I", "-B"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/codex/isolate_agent_test.py"
        ))
        .output()
        .expect("python3 runs isolated agent boundary regressions");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn own_credentials_never_receive_the_managed_placeholder() {
    let launch = render_launch(&AgentOpenRequest::new(
        "agent:agent-7:open:v1",
        options(true, PermissionLevel::Guarded, ReasoningEffort::Minimal),
        None,
    ))
    .expect("own-credential launch");
    assert!(!launch.env.contains_key("CHRONO_UPSTREAM_PLACEHOLDER_KEY"));
    assert!(!launch.env.contains_key("CMA_MODEL_PROXY_PLACEHOLDER"));
}

#[test]
fn acp_open_settings_map_permissions_and_preserve_credential_ownership() {
    let runtime = CodexRuntime {
        config: CodexConfig::default(),
    };
    for (permission, expected_mode) in [
        (PermissionLevel::Guarded, "read-only"),
        (PermissionLevel::Autonomous, "agent-full-access"),
    ] {
        for own_credential in [false, true] {
            let request = AgentOpenRequest::new(
                "agent:agent-7:open:v1",
                options(own_credential, permission, ReasoningEffort::High),
                None,
            );
            let settings = runtime
                .open_settings(&request)
                .expect("Codex ACP settings render");
            assert_eq!(settings.permission_mode.as_deref(), Some(expected_mode));
            assert_eq!(settings.own_credential, own_credential);
        }
    }
}

#[path = "agent_resources_tests.rs"]
mod agent_resources;
