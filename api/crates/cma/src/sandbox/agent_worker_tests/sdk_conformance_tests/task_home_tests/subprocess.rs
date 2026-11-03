use super::*;

const CHILD: &str = "sandbox::agent_worker_tests::sdk_conformance_tests::task_home_tests::subprocess::task_home_create_child";

#[tokio::test]
#[ignore = "invoked only by initial_task_public_service_process_exit_preserves_admission"]
async fn task_home_create_child() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&std::env::var("CMA_TASK_CHILD_DATABASE").unwrap());
    let body: Value =
        serde_json::from_str(&std::env::var("CMA_TASK_CHILD_REQUEST").unwrap()).unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let service = router_components(
        store,
        Arc::new(LiveAgentRegistry::default()),
        &database,
        &client,
        "owner",
        false,
    )
    .await;
    let (status, receipt) = create(&service, &body, "process-restart-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let identity = admitted_identity(&service, &receipt).await;
    println!(
        "TASK_HOME_RECEIPT={}",
        json!({"receipt":receipt,"agent_id":identity})
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_service_process_exit_preserves_admission() {
    let (client, database, harness) = isolated_environment().await;
    let original = "  比较三个工具\n保留原文。  ";
    let body =
        json!({"name":"New task", "workspace_id":harness.workspace.id, "initial_message":original});
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("CMA_TASK_CHILD_DATABASE", database.name())
            .env("CMA_TASK_CHILD_REQUEST", body.to_string())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("1 passed"), "{stdout}");
    let saved: Value = serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("TASK_HOME_RECEIPT="))
            .unwrap(),
    )
    .unwrap();
    let service = router(&harness, &database, &client).await;
    let (status, replay) = create(&service, &body, "process-restart-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
    assert_eq!(replay["id"], saved["receipt"]["id"]);
    let identity = admitted_identity(&service, &replay).await;
    assert_eq!(identity.0, saved["agent_id"].as_str().unwrap());
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            harness
                .worker
                .run_one(&identity, SystemTime::now())
                .await
                .unwrap();
            let transcript: Value = serde_json::from_str(
                &cross_interface_tests::read(
                    &service,
                    &format!("/api/v1/agents/{identity}/transcript"),
                )
                .await,
            )
            .unwrap();
            if let Some(turns) = transcript["turns"]
                .as_array()
                .filter(|turns| !turns.is_empty())
            {
                assert_eq!(turns.len(), 1, "{transcript}");
                assert_eq!(turns[0]["owner_message"], original);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    database.drop().await.unwrap();
}
