use super::*;
use layer2_core::sandbox::agent_io::AgentCommandWorkerStore;
use layer2_core::sandbox::suggestions::SuggestionGenerationService;
use std::path::PathBuf;

mod authority;
pub(super) mod catalog;
mod composition;
mod database;
pub(super) mod home;
mod reader;
mod retained_runtime;
mod workers;

pub(super) async fn run() {
    if std::env::var_os("CMA_INTEGRATED_READER_CONTEXT").is_some() {
        reader::run().await;
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter("cma::sandbox=info,layer3_adapter=info")
        .try_init();
    let directory = PathBuf::from(std::env::var("CMA_DESKTOP_RELAY_DIRECTORY").unwrap());
    let database = database::Database::open().await;
    let mut harness = Harness::with_store_services(
        database.stores.sandbox_model.clone(),
        Arc::new(EmptyAgentSkillResolver),
        1,
        Some(vec![
            ServiceId::new("ornn-api"),
            ServiceId::new("chrono-llm-public"),
        ]),
    )
    .await;
    desktop::configure(&mut harness).await;
    let (credentials, provider_task, provider) = workers::credentials(&database).await;
    let (router, commands) =
        composition::router(&harness, &database, credentials.clone(), &provider).await;
    let (disabled_origin, disabled_server) =
        if std::env::var("CMA_INTEGRATED_POLICY").as_deref() == Ok("1") {
            let (disabled, _) = composition::router_with_policy(
                &harness,
                &database,
                credentials.clone(),
                &provider,
                Default::default(),
            )
            .await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            (
                Some(origin),
                Some(tokio::spawn(async move {
                    axum::serve(listener, disabled).await.unwrap()
                })),
            )
        } else {
            (None, None)
        };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = "Compare three tools, create report.txt with the comparison, and explain one remaining trade-off.";
    let mut body = json!({"name":"New task", "initial_message":task, "desktop":true});
    if std::env::var("CMA_INTEGRATED_REPOSITORY").as_deref() == Ok("1") {
        body["repository"] =
            json!({"url":"https://github.com/ticket19/comparison", "ref":"proof/fixed"});
    }
    tokio::fs::write(directory.join("public-ready.json"), serde_json::to_vec(&json!({
        "origin":origin, "disabled_origin":disabled_origin, "body":body, "database":database.database.name(),
        "control":std::env::var("CMA_INTEGRATED_CONTROL").as_deref() == Ok("1"),
        "boundary":"Synthetic identity/model/provider transport; real Mongo, workers, runtime, native conversation and all Agent public reads."
    })).unwrap()).await.unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (key, receipt) = desktop_native::creation(&body).await;
    let client = reqwest::Client::new();
    let operation = format!(
        "{origin}/api/v1/agent-lifecycle/{}",
        receipt["id"].as_str().unwrap()
    );
    let agent = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let response: Value = client
                .get(&operation)
                .header("authorization", TEST_BEARER)
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_ne!(response["status"], "failed", "{response}");
            if let Some(agent) = response["agent_id"].as_str() {
                break AgentId(agent.into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let detail: Value = client
        .get(format!("{origin}/api/v1/agents/{agent}"))
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["first_message"]["purpose"], "task");
    tokio::fs::write(
        directory.join("agent.json"),
        serde_json::to_vec(&detail).unwrap(),
    )
    .await
    .unwrap();
    let sandbox = SandboxId(detail["sandbox"]["id"].as_str().unwrap().into());
    let workers = workers::start(&harness, &database, sandbox, credentials).await;
    let harness = Arc::new(harness);
    let agent_worker = {
        let harness = harness.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            loop {
                harness
                    .worker
                    .run_one(&agent, SystemTime::now())
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    let command_worker = {
        let store = harness.store.clone();
        tokio::spawn(async move {
            loop {
                for pending in store
                    .command_work_page(&PageRequest::default())
                    .await
                    .unwrap()
                    .items
                {
                    commands
                        .run_one(&pending.receipt.envelope.command_id)
                        .await
                        .unwrap();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    let suggestions = SuggestionGenerationService::new(harness.store.clone());
    let generator = crate::sandbox::suggestions::HttpSuggestionGenerator::new(
        &std::env::var("CMA_INTEGRATED_MODEL_ORIGIN").unwrap(),
        "proof-model",
        crate::auth::persistence::SecretValue::new("local-model-authority".into()),
        None,
    )
    .unwrap();
    let suggestion_worker = tokio::spawn(async move {
        loop {
            suggestions.run_next(&generator).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let result = tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            if let Ok(bytes) = tokio::fs::read(directory.join("web-result.json")).await {
                break serde_json::from_slice::<Value>(&bytes).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    agent_worker.abort();
    command_worker.abort();
    suggestion_worker.abort();
    provider_task.abort();
    for worker in workers {
        worker.abort();
    }
    let result = result.expect("the real composed Web journey must finish");
    assert_eq!(result["storyComplete"], true, "{result}");
    let history: Value = client
        .get(format!("{origin}/api/v1/agents/{agent}/transcript"))
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let extended = std::env::var("CMA_INTEGRATED_EXTENDED").as_deref() == Ok("1");
    assert_eq!(
        history["turns"].as_array().unwrap().len(),
        if extended { 3 } else { 2 },
        "{history}"
    );
    assert_eq!(history["turns"][0]["owner_message"], task);
    assert_eq!(history["turns"][1]["owner_message"], result["edited"]);
    if extended {
        assert_eq!(
            history["turns"][2]["owner_message"],
            "Compare maintenance effort for the three tools."
        );
    }
    if std::env::var("CMA_INTEGRATED_READER").as_deref() == Ok("1") {
        reader::verify(
            &directory, &database, &harness, &body, &key, &receipt, &agent,
        )
        .await;
    }
    if body.get("repository").is_some() {
        let repository: Value = serde_json::from_slice(
            &tokio::fs::read(directory.join("repository.json"))
                .await
                .unwrap(),
        )
        .unwrap();
        let detail: Value = client
            .get(format!("{origin}/api/v1/agents/{agent}"))
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
            detail["workspace"]["source"]["source_commit"],
            repository["commit"]
        );
        assert_eq!(
            detail["workspace"]["resolved_commit"],
            detail["worktree"]["base_commit"]
        );
        tokio::fs::write(
            directory.join("repository-public.json"),
            serde_json::to_vec_pretty(&detail).unwrap(),
        )
        .await
        .unwrap();
    }
    tokio::fs::write(directory.join("public-result.json"), serde_json::to_vec_pretty(&json!({
        "passed":true, "agent_id":agent, "request_key":key, "receipt":receipt, "history":history,
        "database":database.database.name()
    })).unwrap()).await.unwrap();
    tokio::fs::write(directory.join("stop.json"), b"{}")
        .await
        .unwrap();
    server.abort();
    if let Some(server) = disabled_server {
        server.abort();
    }
}
