//! Real executable fixture, addressed through the production hosted transport.
use super::*;
use layer3_adapter::hosted::{
    HostedAgentOpenSettings, HostedControlTransport, ProviderHostedControlTransport,
};
use layer3_adapter::wire::{FencedRequest, RequestFrame, ResponseFrame};
use layer3_adapter::{
    AgentHomePlan, AgentLaunch, AgentSlot, CredentialFrame, HomeFile, InstallRequest,
    RuntimeCredential, RuntimeHandle, RuntimeStatus, RuntimeStatusRequest, WorkspaceRequest,
    WorkspaceSource, WorktreeRequest,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};

pub(super) struct Native {
    child: tokio::process::Child,
    pub manifest: serde_json::Value,
    pub transport: ProviderHostedControlTransport,
    pub handle: RuntimeHandle,
    pub channel: layer3_adapter::ConversationChannel,
    pub updates: tokio::sync::mpsc::Receiver<layer3_adapter::ConversationUpdate>,
    pub cwd: PathBuf,
    pub agent_home: PathBuf,
}

impl Native {
    pub async fn start() -> Self {
        tokio::time::timeout(Duration::from_secs(60), Self::start_inner())
            .await
            .expect("native setup deadline")
    }

    async fn start_inner() -> Self {
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.github/scripts/worker-runtime-fixture.mjs");
        let mut command = tokio::process::Command::new("node");
        command
            .arg(script)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap());
        for key in [
            "CMA_CODEX_TEST_BINARY",
            "CMA_ACP_TEST_BRIDGE",
            "CMA_TEST_RUNTIME_BINARY",
            "CMA_SUPERVISION_EVIDENCE_DIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(false)
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(40), stdout.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let manifest: serde_json::Value =
            serde_json::from_str(&line).expect("native fixture started");
        println!("native runtime evidence: {}", manifest["root"]);
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            layer3_adapter::provider::SandboxClient::new(
                manifest["origin"].as_str().unwrap(),
                "loopback-supervision-provider",
            ),
        )
        .unwrap();
        let handle = RuntimeHandle::new("runtime-proof");
        transport
            .bind_runtime(&handle, "sbx_commands")
            .await
            .unwrap();
        let exchange = |request| transport.exchange(&handle, request);
        assert!(matches!(
            exchange(RequestFrame::Install(InstallRequest::new(
                "proof-install",
                vec![]
            )))
            .await
            .unwrap(),
            ResponseFrame::Install(_)
        ));
        assert!(matches!(
            exchange(RequestFrame::Credential(CredentialFrame {
                operation_id: "proof-install".into(),
                value: RuntimeCredential::new(format!("local-proof-{}", uuid::Uuid::new_v4())),
                custody_generation: Some(1),
                github_route: None,
            }))
            .await
            .unwrap(),
            ResponseFrame::Credential(_)
        ));
        let prepared = exchange(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:workspace_proof:prepare:v1",
            "workspace_proof",
            "project",
            WorkspaceSource::Init,
        )))
        .await
        .unwrap();
        assert!(
            matches!(prepared, ResponseFrame::PrepareWorkspace(_)),
            "{prepared:?}"
        );
        let tree = exchange(RequestFrame::AddWorktree(FencedRequest::new(
            1,
            WorktreeRequest::new("worktree:project:main:add:v1", "project", "main", "HEAD"),
        )))
        .await
        .unwrap();
        let ResponseFrame::AddWorktree(tree) = tree else {
            panic!("{tree:?}")
        };
        let cwd = PathBuf::from(&tree.receipt.path);
        let home =
            PathBuf::from(manifest["home"].as_str().unwrap()).join(".cma/agents/agt_commands/home");
        let config = format!("model = \"gpt-6\"\nmodel_provider = \"cma_test\"\nweb_search = \"disabled\"\napproval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n[model_providers.cma_test]\nname = \"Supervision proof\"\nbase_url = \"{}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n", manifest["origin"].as_str().unwrap());
        let mut plan = AgentHomePlan {
            operation_id: "agent:agt_commands:home:create:v1".into(),
            request_sha256: String::new(),
            agent_id: AgentSlot::new("agt_commands"),
            files: vec![HomeFile {
                path: "config.toml".into(),
                bytes: config.into_bytes(),
            }],
            symlinks: vec![],
            skills: vec![],
        };
        plan.request_sha256 = plan.payload_sha256();
        assert!(matches!(
            exchange(RequestFrame::MaterializeAgentHome(FencedRequest::new(
                1, plan
            )))
            .await
            .unwrap(),
            ResponseFrame::MaterializeAgentHome(_)
        ));
        let mut launch = AgentLaunch {
            operation_id: "agent:agt_commands:open:v1".into(),
            request_sha256: String::new(),
            agent_id: AgentSlot::new("agt_commands"),
            cwd: cwd.to_str().unwrap().into(),
            home: home.to_str().unwrap().into(),
            program: manifest["node"].as_str().unwrap().into(),
            args: vec![manifest["bridge"].as_str().unwrap().into()],
            env: BTreeMap::from([("CODEX_HOME".into(), home.to_str().unwrap().into())]),
        };
        launch.request_sha256 = launch.payload_sha256();
        let settings = HostedAgentOpenSettings {
            coordinated_ingress: false,
            retained_json: false,
            bounded_raw_output: false,
            bounded_permission_state: false,
            output_json_budget: false,
            shared_runtime_output: false,
            already_initialized_error: Some(
                serde_json::json!({"code":-32603,"message":"Internal error","data":{"details":"Already initialized"}}),
            ),
            open_completion: Some(layer3_adapter::hosted::HostedOpenCompletionProtocol::V1),
            context_refresh: None,
            permission_mode: Some("agent-full-access".into()),
            // The loopback model explicitly requires no OpenAI account.
            own_credential: false,
            channel_tool: None,
            provider_tool: None,
            access_tool: None,
            recovered_prompt: None,
            recovered_context_sequence: None,
        };
        let (sender, updates) = tokio::sync::mpsc::channel(256);
        let (channel, _) = transport
            .open_agent(
                &handle,
                FencedRequest::new(1, launch),
                None,
                settings,
                sender,
            )
            .await
            .unwrap();
        Self {
            child,
            manifest,
            transport,
            handle,
            channel,
            updates,
            cwd,
            agent_home: home,
        }
    }

    pub async fn state(&self) -> serde_json::Value {
        let state: serde_json::Value = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!(
                "{}/proof/state",
                self.manifest["origin"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(state["error"].is_null(), "{state}");
        state
    }

    pub async fn release(&self, part: &str) {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!(
                "{}/proof/release-{part}",
                self.manifest["origin"].as_str().unwrap()
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    pub async fn status(&self) -> RuntimeStatus {
        let response = self
            .transport
            .exchange(
                &self.handle,
                RequestFrame::RuntimeStatus(RuntimeStatusRequest {
                    sandbox_id: "sbx_commands".into(),
                    generation: 1,
                    home_identity: "a".repeat(43),
                }),
            )
            .await
            .unwrap();
        let ResponseFrame::RuntimeStatus(status) = response else {
            panic!("{response:?}")
        };
        status
    }

    pub async fn close(mut self) {
        let closed = tokio::time::timeout(
            Duration::from_secs(5),
            self.transport.exchange(
                &self.handle,
                RequestFrame::CloseAgent(FencedRequest::new(
                    1,
                    layer3_adapter::AgentCloseRequest::new(
                        "agent:agt_commands:close:v1",
                        AgentSlot::new("agt_commands"),
                    ),
                )),
            ),
        )
        .await;
        let _ = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!(
                "{}/proof/stop",
                self.manifest["origin"].as_str().unwrap()
            ))
            .send()
            .await;
        self.child.stdin.take();
        let exited = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        assert!(
            matches!(closed, Ok(Ok(ResponseFrame::CloseAgent(_)))),
            "close agent: {closed:?}"
        );
        assert!(exited.unwrap().unwrap().success());
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // EOF asks the fixture to close its runtime and release only its own work.
        self.child.stdin.take();
    }
}
