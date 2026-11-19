use super::*;

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_legacy_complete_result_refuses_oversize_but_every_output_page_remains_available() {
    let (mut f, id) = retained(1_000).await;
    for seq in 993..=1000 {
        let row = AgentLogEntry {
            agent_id: f.agent.id.clone(),
            seq,
            appended_at: NOW.into(),
            kind: AgentLogKind::AgentMessage {
                content: "x".repeat(600_000),
            },
        };
        f.store
            .agent_log
            .replace_one(
                doc! {"agent_id":f.agent.id.as_str(),"seq":seq as i64},
                log_document(&row).unwrap(),
            )
            .await
            .unwrap();
    }
    f.agent.revision += 1;
    f.store
        .agents
        .replace_one(
            doc! {"_id":f.agent.id.as_str()},
            crate::store::agents::agent_document(&f.agent).unwrap(),
        )
        .await
        .unwrap();
    f.store = f
        .store
        .prepare_bounded_admission(std::slice::from_ref(&f.agent.id))
        .await
        .unwrap();
    let servers = servers_mode(&f, true).await;
    let client = client();
    let path = format!("/api/v2/responses/{id}");
    let response = request(&client, &f, &servers[0], &path)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("response_pagination_required"));
    let mut cursor = None;
    let mut count = 0;
    loop {
        let path = match &cursor {
            Some(cursor) => format!("{path}/output?after={cursor}"),
            None => format!("{path}/output"),
        };
        let response = request(&client, &f, &servers[count % 2], &path)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let bytes = response.bytes().await.unwrap();
        assert!(bytes.len() <= 4 * 1024 * 1024);
        let page: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(page["data"].as_array().unwrap().len(), 1);
        assert_eq!(
            page["data"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .len(),
            600_000
        );
        count += 1;
        if page["has_more"] == false {
            break;
        }
        cursor = Some(page["next_after"].as_str().unwrap().to_owned());
    }
    assert_eq!(count, 8);
    drop(servers);
    f.close().await;
}
