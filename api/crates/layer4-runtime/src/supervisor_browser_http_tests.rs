use super::*;
use layer3_adapter::{
    AgentHomePlan, AgentLaunch, AgentSlot, WorkspaceRequest, WorkspaceSource, WorktreeRequest,
};
use std::{collections::BTreeMap, time::Duration};

#[tokio::test]
#[ignore = "requires explicit real Chrome and pinned bridge paths"]
async fn real_browser_enablement_survives_cancelled_http_observation_without_resetting_context() {
    let binary = std::env::var("CMA_BROWSER_BINARY").expect("explicit real browser required");
    let bridge = std::env::var("CMA_ACP_TEST_BRIDGE").expect("explicit pinned bridge required");
    let provider =
        std::env::var("CMA_CODEX_TEST_BINARY").expect("explicit pinned native provider required");
    let fixture = FilesystemFixture(std::env::temp_dir().join(format!(
        "cma-browser-http-{}",
        uuid::Uuid::new_v4().simple()
    )));
    let mut config = RuntimeConfig::for_test(fixture.0.clone());
    config
        .process_env
        .insert("CMA_BROWSER_BINARY".into(), binary);
    config
        .process_env
        .insert("PATH".into(), std::env::var("PATH").unwrap());
    let agent_home = config
        .agent_home("browser-agent")
        .to_string_lossy()
        .into_owned();
    let supervisor = Supervisor::new(config, SupervisorBoundary::default()).unwrap();
    let router = crate::link::router(supervisor.clone());
    exchange(
        &router,
        serde_json::to_value(RequestFrame::Install(InstallRequest::new(
            "browser-install",
            Vec::new(),
        )))
        .unwrap(),
    )
    .await;
    exchange(
        &router,
        serde_json::to_value(RequestFrame::Credential(CredentialFrame {
            operation_id: "browser-credential".into(),
            value: RuntimeCredential::new("local-test"),
            custody_generation: Some(1),
            github_route: None,
        }))
        .unwrap(),
    )
    .await;
    let workspace = exchange(
        &router,
        serde_json::to_value(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_browser:prepare:v1",
            "wks_browser",
            "browser-project",
            WorkspaceSource::Init,
        )))
        .unwrap(),
    )
    .await;
    let worktree = WorktreeRequest::new(
        "worktree:browser-project:browser-agent:add:v1",
        "browser-project",
        "browser-agent",
        workspace["resolved_commit"].as_str().unwrap(),
    );
    let worktree = exchange(
        &router,
        json!({"t":"add_worktree", "authority_fence":1, "request":worktree}),
    )
    .await;
    let mut home = AgentHomePlan {
        operation_id: "agent:browser-agent:home:create:v1".into(),
        request_sha256: String::new(),
        agent_id: AgentSlot("browser-agent".into()),
        files: Vec::new(),
        symlinks: Vec::new(),
        skills: Vec::new(),
    };
    home.request_sha256 = home.payload_sha256();
    let materialized = exchange(
        &router,
        json!({"t":"materialize_agent_home", "authority_fence":1, "request":home}),
    )
    .await;
    assert_eq!(materialized["t"], "materialize_agent_home");
    let mut launch = AgentLaunch {
        operation_id: "agent:browser-agent:open:v1".into(),
        request_sha256: String::new(),
        agent_id: AgentSlot("browser-agent".into()),
        cwd: worktree["receipt"]["path"].as_str().unwrap().into(),
        home: agent_home,
        program: "node".into(),
        args: vec![bridge],
        env: BTreeMap::from([("CODEX_PATH".into(), provider)]),
    };
    launch.request_sha256 = launch.payload_sha256();
    assert_eq!(
        exchange(
            &router,
            json!({"t":"open_agent", "authority_fence":1, "request":launch})
        )
        .await["t"],
        "open_agent"
    );
    let mut request = json!({"t":"browser", "sandbox_id":"sandbox-test", "runtime_generation":1,
        "boot_id":supervisor.boot_id, "agent_id":"browser-agent", "operation":"enable"});
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        exchange(&router, request.clone())
    )
    .await
    .is_err());
    request["operation"] = json!("observe");
    request["observer"] = json!("reconnected-observer");
    let recovered = exchange(&router, request.clone()).await;
    let mut disconnected = request.clone();
    disconnected["operation"] = json!("release");
    let released = exchange(&router, disconnected).await;
    let observed_again = exchange(&router, request.clone()).await;
    let acquired = exchange(&router, json!({"t":"browser_control", "sandbox_id":"sandbox-test", "runtime_generation":1,
        "boot_id":supervisor.boot_id, "agent_id":"browser-agent", "context_id":recovered["observation"]["context_id"],
        "actor":"owner-proof", "client_id":"http-proof", "command":"acquire", "expected_revision":0,
        "operation_id":"78af4405-b644-44ab-b58d-53ce4d62acc0"})).await;
    assert_eq!(acquired["t"], "browser_control", "{acquired}");
    assert_eq!(acquired["control"]["state"], "user");
    request["operation"] = json!("disable");
    exchange(&router, request).await;
    assert_eq!(recovered["t"], "browser", "{recovered}");
    assert_eq!(recovered["observation"]["state"], "live");
    assert_eq!(released["observation"]["state"], "ready");
    assert_eq!(
        observed_again["observation"]["context_id"],
        recovered["observation"]["context_id"]
    );
    assert_eq!(
        observed_again["observation"]["frame"]["data"],
        recovered["observation"]["frame"]["data"]
    );
}
