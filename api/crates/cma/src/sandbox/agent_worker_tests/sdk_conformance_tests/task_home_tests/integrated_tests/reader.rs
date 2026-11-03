use super::*;
use layer3_adapter::hosted::HostedBackend;
use std::path::Path;

pub(super) async fn verify(
    directory: &Path,
    database: &database::Database,
    harness: &Harness<MongoSandboxStore>,
    body: &Value,
    key: &str,
    receipt: &Value,
    agent: &AgentId,
) {
    let stored = harness.store.get_agent(agent).await.unwrap().unwrap();
    let sandbox = harness
        .store
        .get_sandbox(&stored.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let handle = RuntimeHandle(sandbox.runtime.as_ref().unwrap().runtime_id.clone());
    let observation = harness
        .backend
        .inventory()
        .await
        .unwrap()
        .into_iter()
        .find(|value| value.handle == handle)
        .unwrap();
    let request = harness.backend.snapshot(&handle).await.unwrap().request;
    let control = tokio::fs::read(directory.join("control-result.json"))
        .await
        .ok()
        .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap());
    let context = json!({
        "connection":database.connection, "encryption":database.encryption,
        "database":database.database.name(), "agent":agent, "workspace":stored.workspace_id,
        "body":body, "key":key, "receipt":receipt, "observation":observation,
        "provider_request":request,
        "control":control, "expected_turns":if std::env::var("CMA_INTEGRATED_EXTENDED").as_deref() == Ok("1") {3} else {2},
    });
    let binary = std::env::var_os("CMA_INTEGRATED_READER_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap());
    let name = "sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::environment::initial_task_public_default_environment_delivers_exact_task_after_preparation";
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(&binary)
            .args(["--exact", name, "--ignored", "--nocapture"])
            .env("CMA_INTEGRATED_READER_CONTEXT", context.to_string())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    tokio::fs::write(directory.join("reader-process.log"), &log)
        .await
        .unwrap();
    assert!(output.status.success(), "retained reader failed: {log}");
    assert!(log.contains("1 passed; 0 failed;"), "{log}");
}

async fn retained_harness(
    database: &database::Database,
    context: &Value,
) -> Harness<MongoSandboxStore> {
    let store = database.stores.sandbox_model.clone();
    let workspace = store
        .get_workspace(&WorkspaceId(context["workspace"].as_str().unwrap().into()))
        .await
        .unwrap()
        .unwrap();
    let sandbox = store
        .get_sandbox(&workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let runtime = RuntimeHandle(sandbox.runtime.as_ref().unwrap().runtime_id.clone());
    let registry = Arc::new(LiveAgentRegistry::default());
    let updates = DurableAgentUpdateRecorder::new(store.clone());
    let backend = InMemoryHostedBackend::default();
    let adapter = retained_runtime::adapter(&sandbox, context).await;
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: updates.clone(),
            operations: AgentOperationService::new(store.clone(), registry.clone()),
        },
        AgentWorkerConfig::new("ticket19-retained-reader-no-worker", DEPLOYMENT_ID),
    );
    Harness {
        store,
        worker,
        registry,
        backend,
        adapter,
        runtime,
        workspace,
        now: SystemTime::now(),
        updates,
    }
}

pub(super) async fn run() {
    let context: Value =
        serde_json::from_str(&std::env::var("CMA_INTEGRATED_READER_CONTEXT").unwrap()).unwrap();
    let directory = PathBuf::from(std::env::var("CMA_DESKTOP_RELAY_DIRECTORY").unwrap());
    let database = database::Database::reopen(&context).await;
    let harness = retained_harness(&database, &context).await;
    let (credentials, provider_task, provider) = workers::credentials(&database).await;
    let (router, _) = composition::router(&harness, &database, credentials, &provider).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    let create = client
        .post(format!("{origin}/api/v1/agents"))
        .header("authorization", TEST_BEARER)
        .header("idempotency-key", context["key"].as_str().unwrap())
        .json(&context["body"])
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::ACCEPTED);
    let receipt: Value = create.json().await.unwrap();
    assert_eq!(receipt["id"], context["receipt"]["id"]);
    let agent = context["agent"].as_str().unwrap();
    let mut reads = serde_json::Map::new();
    for suffix in [
        "",
        "/transcript",
        "/message-feedback",
        "/environment",
        "/files/content?path=cma%2Freport.txt",
    ] {
        let response = client
            .get(format!("{origin}/api/v1/agents/{agent}{suffix}"))
            .header("authorization", TEST_BEARER)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let value: Value = response.json().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{suffix}: {value}");
        reads.insert(suffix.into(), value);
    }
    assert_eq!(
        reads["/transcript"]["turns"].as_array().unwrap().len(),
        context["expected_turns"].as_u64().unwrap() as usize
    );
    assert_eq!(reads["/message-feedback"]["items"][0]["vote"], "down");
    assert_eq!(
        reads["/files/content?path=cma%2Freport.txt"]["sha256"],
        "da70a76eb596386fcf3902dcf9292db26e1b71a189afe55674bca82a698f70f4"
    );
    let download = client
        .get(format!(
            "{origin}/api/v1/agents/{agent}/files/download?path=cma%2Freport.txt"
        ))
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        download.bytes().await.unwrap().as_ref(),
        b"Tool A: simple\nTool B: flexible\nTool C: portable\n"
    );
    let message = reads["/message-feedback"]["items"][0]["message_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let reasons: Value = client
        .get(format!(
            "{origin}/api/v1/agents/{agent}/messages/{message}/feedback"
        ))
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        reasons["comment"],
        "Please explain the flexibility trade-off."
    );
    reads.insert("reasons".into(), reasons);
    if !context["control"].is_null() {
        let scope = &context["control"]["scope"];
        assert_eq!(
            reads["/environment"]["runtime_incarnation"],
            scope["runtime_incarnation"]
        );
        let browser: Value = client
            .get(format!("{origin}/api/v1/agents/{agent}/browser"))
            .query(&[(
                "runtime_incarnation",
                scope["runtime_incarnation"].as_str().unwrap(),
            )])
            .header("authorization", TEST_BEARER)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(browser["context_id"], context["control"]["context"]);
        reads.insert("browser".into(), browser);
        let control: Value = client
            .get(format!("{origin}/api/v1/agents/{agent}/browser/control"))
            .query(scope)
            .header("authorization", TEST_BEARER)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(control["context_id"], context["control"]["context"]);
        assert_eq!(control["state"], "agent");
        reads.insert("control".into(), control);
    }
    tokio::fs::write(
        directory.join("reader-result.json"),
        serde_json::to_vec_pretty(&json!({
            "passed":true, "pid":std::process::id(), "executable":std::env::current_exe().unwrap(),
            "receipt":receipt, "agent":agent, "reads":reads,
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    server.abort();
    provider_task.abort();
}
