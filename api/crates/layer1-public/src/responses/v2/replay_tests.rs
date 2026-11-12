use super::*;

#[tokio::test]
async fn replay_ids_are_stable_and_queue_failure_is_durable_and_retrievable() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let (status, bytes) = fixtures::response(
        &f.router,
        request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let full = events(&bytes);
    assert_eq!(full.len(), 2);
    assert_eq!(full[1]["type"], "response.failed");
    let (_, replay) = fixtures::response(
        &f.router,
        request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events?starting_after=0"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(events(&replay), full[1..]);
    assert!(std::str::from_utf8(&replay).unwrap().contains("id: 1"));
    let (_, retrieved) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "owner-a",
        None,
    )
    .await;
    assert_eq!(retrieved, full[1]["response"]);
}

#[tokio::test]
async fn shared_retrieval_redacts_metadata_and_revocation_fences_buffered_frames() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let share = f
        .access
        .change_share(
            &context("owner-a"),
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_fixture".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "share".into(),
            },
        )
        .await
        .unwrap()
        .share;
    let (status, shared) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "reader",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shared["metadata"], json!({}));
    let record = f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .remove(0);
    f.store
        .reject_unclaimed_command(
            &record.receipt.envelope.command_id,
            record.revision,
            CommandFailureCode::AuthorityRevoked,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "reader",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    f.access
        .change_share(
            &context("owner-a"),
            None,
            ShareCommand {
                selector: share.selector,
                change: ShareChange::Revoke { share_id: share.id },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke".into(),
            },
        )
        .await
        .unwrap();
    let failure = body.next().await.unwrap().unwrap();
    let event = &events(&failure)[0];
    assert_eq!(event["type"], "error");
    assert!(event.get("response").is_none());
    assert!(!std::str::from_utf8(&failure).unwrap().contains("id: "));
    assert!(body.next().await.is_none());
    assert_eq!(
        read_json(
            &f.router,
            Method::GET,
            &format!("/api/v2/responses/{id}"),
            "reader",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(start_paused = true)]
async fn idle_shared_stream_expires_without_a_poll_renewing_membership() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    f.access
        .change_share(
            &context("owner-a"),
            Some("fixture-credential"),
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: AgentId("agt_fixture".into()),
                },
                change: ShareChange::Create {
                    target_org_id: SubjectId::new("org"),
                    mode: ShareMode::View,
                },
                expected_share_revision: None,
                idempotency_key: "share".into(),
            },
        )
        .await
        .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events"),
            "reader",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert_eq!(
        events(&body.next().await.unwrap().unwrap())[0]["type"],
        "response.created"
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    let event = events(&body.next().await.unwrap().unwrap());
    assert_eq!(event[0]["type"], "error");
    assert!(body.next().await.is_none());
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items[0]
            .status,
        AgentResponseStatus::Queued
    );
}

#[tokio::test]
async fn replay_refuses_ambiguous_cursors_and_invalid_queries_without_changing_work() {
    let f = fixture().await;
    let response = create(&f, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let command = layer2_core::ids::AgentCommandId(
        response.headers()["x-cma-command-id"]
            .to_str()
            .unwrap()
            .into(),
    );
    drop(response);
    let before = f.store.get_command(&command).await.unwrap();
    for (query, headers) in [
        ("", vec!["0", "1"]),
        ("", vec!["invalid"]),
        ("?starting_after=0", vec!["1"]),
        ("?unknown=true", vec![]),
        ("?starting_after=0&starting_after=1", vec![]),
        ("?starting_after=99", vec![]),
    ] {
        let mut req = request(
            Method::GET,
            &format!("/api/v2/responses/{id}/events{query}"),
            "owner-a",
            None,
            None,
        );
        for header in headers {
            req.headers_mut()
                .append("last-event-id", header.parse().unwrap());
        }
        let (status, body) = fixtures::json_response(&f.router, req).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["error"]["code"], "invalid_replay_cursor");
    }
    for query in [
        "?after=missing&before=other",
        "?before=missing",
        "?unknown=true",
    ] {
        let (status, _) = read_json(
            &f.router,
            Method::GET,
            &format!("/api/v2/conversations/agt_fixture/items{query}"),
            "owner-a",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(f.store.get_command(&command).await.unwrap(), before);
}
