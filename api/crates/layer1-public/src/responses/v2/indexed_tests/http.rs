use super::*;

#[tokio::test]
async fn legacy_http_pages_oversized_output_without_truncating_the_response() {
    let mut kinds = beginning();
    kinds.extend((0..12).map(|_| message(&"a".repeat(400_000))));
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let f = make(kinds).await;
    let router = crate::responses::router(f.state.clone());
    let path = "/api/v1/responses/resp_agt_indexed_3";
    let (status, body) = crate::rest::agents_tests::json_response(
        &router,
        request(Method::GET, path, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "response_pagination_required");
    let mut from = 0;
    let mut count = 0;
    loop {
        let (status, body) = crate::rest::agents_tests::json_response(
            &router,
            request(
                Method::GET,
                &format!("{path}?output_page=true&output_after={from}"),
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(serde_json::to_vec(&body).unwrap().len() <= 4 * 1024 * 1024);
        count += body["data"].as_array().unwrap().len();
        if body["has_more"] == false {
            break;
        }
        let next = body["next_after"].as_u64().unwrap();
        assert!(next > from);
        from = next;
    }
    assert_eq!(count, 12);
    let (status, _) = crate::rest::agents_tests::json_response(
        &router,
        request(
            Method::GET,
            &format!("{path}?output_page=true&output_after=99"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert!(!status.is_success());
}

#[tokio::test]
async fn legacy_indexed_stream_retains_intermediate_tools_and_fences_buffered_frames() {
    let mut kinds = beginning();
    kinds.extend([
        tool("call", "running"),
        output("call", "first"),
        update("call", "completed"),
        output("call", "latest"),
        message("answer"),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let f = make(kinds).await;
    let response = crate::responses::stream::open_authorized(
        f.state.clone(),
        crate::AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        },
        f.agent.clone(),
        3,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut all = vec![];
    while let Some(frame) = stream.next().await {
        all.extend(events(&frame.unwrap()));
    }
    assert_eq!(all.last().unwrap()["type"], "response.completed");
    let activity = all
        .iter()
        .filter(|e| e["type"] == "response.cma_chat.tool_activity")
        .collect::<Vec<_>>();
    assert_eq!(activity.len(), 4);
    assert_eq!(activity[1]["item"]["output"]["text"], "first");
    assert_eq!(activity[3]["item"]["output"]["text"], "latest");
    let response = crate::responses::stream::open_authorized(
        f.state.clone(),
        crate::AuthenticatedResourceCaller {
            context: context("owner-a"),
            credential: Some(crate::DelegatedCredential::new("fixture-credential".into())),
        },
        f.agent.clone(),
        3,
        None,
    )
    .await;
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(
        events(&stream.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    let mut deleted = f.agent.clone();
    deleted.status = AgentStatus::Deleted;
    deleted.deleted_at = Some(AT.into());
    deleted.revision += 1;
    f.store.save_agent(f.agent.revision, deleted).await.unwrap();
    assert!(
        stream.next().await.is_none(),
        "buffered frames cannot escape deleted authority"
    );
}
