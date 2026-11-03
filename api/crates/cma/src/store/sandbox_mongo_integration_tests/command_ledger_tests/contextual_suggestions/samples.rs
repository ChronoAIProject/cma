use super::*;

#[tokio::test]
#[ignore = "requires the explicitly authorized bounded real-model proxy and isolated Mongo"]
async fn contextual_suggestions_real_samples_are_saved_and_read_through_the_public_route() {
    let proxy =
        std::env::var("CMA_SUGGESTION_SAMPLE_PROXY").expect("explicit local sample proxy required");
    assert!(proxy.starts_with("http://127.0.0.1:"));
    let model =
        std::env::var("CMA_SUGGESTION_SAMPLE_MODEL").expect("explicit sample model required");
    let output =
        std::env::var("CMA_SUGGESTION_SAMPLE_OUTPUT").expect("external evidence path required");
    let (client, name) = database("ticket13_real_samples").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let generator = HttpSuggestionGenerator::new(
        &proxy,
        &model,
        SecretValue::new("synthetic-suggestion-authority".into()),
        None,
    )
    .unwrap();
    let cases = [
        ("comparison", "Compare SQLite and PostgreSQL for an offline desktop notes app used by one person. I want minimal maintenance.", "SQLite fits this offline single-user app: it needs no server and stores data locally. PostgreSQL helps with concurrent clients but adds server maintenance you do not need."),
        ("document_edit", "Shorten this introduction for new volunteers: Our neighborhood food pantry provides groceries to residents each Thursday. Volunteers sort food, welcome visitors and pack bags. Keep the Thursday detail.", "Our neighborhood pantry gives residents groceries every Thursday. Volunteers sort food, welcome visitors and pack bags."),
        ("clarification", "帮我制定一个学习 Python 的四周计划。", "需要先确认你的 Python 基础：你是从未编程，还是已经能写简单脚本？目前无法据此确定每天的学习内容。"),
        ("closed", "What is 17 times 4? Only the number, please.", "68"),
        ("false_artifact", "Draft a one-sentence event announcement here in chat. Do not create or publish files.", "I created announcement.pdf and published the event page. Download the finished file from the project directory."),
    ];
    let mut evidence = Vec::new();
    for (case, goal, answer) in cases {
        let (_, agent) = seed(&store, case, goal, answer).await;
        assert!(SuggestionGenerationService::new(store.clone())
            .run_next(&generator)
            .await
            .unwrap());
        let routes = router(store.clone());
        let (status, saved) = read(&routes, &agent, "owner").await;
        assert_eq!(status, StatusCode::OK);
        let replica = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        assert_eq!(read(&router(replica), &agent, "owner").await.1, saved);
        evidence.push(serde_json::json!({"case":case,"goal":goal,"answer":answer,"confirmed_resources":[],"saved_public_read":saved}));
        std::fs::write(&output, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        assert!(
            matches!(
                evidence.last().unwrap()["saved_public_read"]["status"].as_str(),
                Some("ready" | "empty")
            ),
            "sample {case} did not return usable generation evidence"
        );
    }
    assert!(!SuggestionGenerationService::new(store)
        .run_next(&generator)
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}
