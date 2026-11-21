use super::*;

#[tokio::test]
async fn message_feedback_reasons_require_authenticated_owner_and_canonical_final_answer() {
    let (fixture, _, _) = feedback_fixture().await;
    let body = json!({"expected_version":1,"request_id":"owner-request","reasons":["other"]});
    for (method, suffix, payload) in [
        (Method::GET, "", None),
        (Method::PUT, "/reasons", Some(body.clone())),
    ] {
        let path = format!("{AGENT_MESSAGE}{suffix}");
        let (status, _) = json_response(
            &fixture.router,
            request(method.clone(), &path, OTHER, None, payload.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let mut forbidden = request(method.clone(), &path, OWNER, None, payload.clone());
        forbidden.extensions_mut().insert(Authorization::new([]));
        assert_eq!(
            response(&fixture.router, forbidden).await.0,
            StatusCode::FORBIDDEN
        );
        let mut anonymous = request(method.clone(), &path, OWNER, None, payload.clone());
        anonymous.extensions_mut().remove::<AuthenticatedSubject>();
        assert_eq!(
            response(&fixture.router, anonymous).await.0,
            StatusCode::UNAUTHORIZED
        );
        for message in ["event:8", "event:09", "optimistic:9"] {
            let (status, _) = json_response(
                &fixture.router,
                request(
                    method.clone(),
                    &format!("/api/v1/agents/agt_feedback/messages/{message}/feedback{suffix}"),
                    OWNER,
                    None,
                    payload.clone(),
                ),
            )
            .await;
            assert_eq!(
                status,
                if message == "event:8" {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::UNPROCESSABLE_ENTITY
                }
            );
        }
    }
}

#[tokio::test]
async fn message_feedback_reasons_legacy_clear_then_down_refuses_old_version_and_preserves_history()
{
    for vote in [json!("up"), Value::Null] {
        let (fixture, feedback, _) = feedback_fixture().await;
        let transcript = "/api/v1/agents/agt_feedback/transcript";
        let before = json_response(
            &fixture.router,
            request(Method::GET, transcript, OWNER, None, None),
        )
        .await;
        json_response(
            &fixture.router,
            request(
                Method::PUT,
                AGENT_MESSAGE,
                OWNER,
                None,
                Some(json!({"vote":"down"})),
            ),
        )
        .await;
        let path = format!("{AGENT_MESSAGE}/reasons");
        let body = json!({"expected_version":1,"request_id":"six-reasons","reasons":["inaccurate","unanswered","unusable_steps","incomplete","unclear","other"],"comment":"A saved description"});
        let (status, saved) = json_response(
            &fixture.router,
            request(Method::PUT, &path, OWNER, None, Some(body.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let reopened = agent_router(fixture.state.clone().with_message_feedback(feedback));
        let (_, current) = json_response(
            &reopened,
            request(Method::GET, AGENT_MESSAGE, OWNER, None, None),
        )
        .await;
        assert_eq!(current, saved);
        for next in [vote.clone(), json!("down")] {
            let (status, _) = json_response(
                &reopened,
                request(
                    Method::PUT,
                    AGENT_MESSAGE,
                    OWNER,
                    None,
                    Some(json!({"vote":next})),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let (status, _) = json_response(
                &reopened,
                request(Method::PUT, &path, OWNER, None, Some(body.clone())),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT);
            let (_, current) = json_response(
                &reopened,
                request(Method::GET, AGENT_MESSAGE, OWNER, None, None),
            )
            .await;
            assert_eq!(current["vote"], next);
            assert_eq!(current["reasons"], json!([]));
            assert_eq!(current["comment"], "");
        }
        assert_eq!(
            json_response(
                &reopened,
                request(Method::GET, transcript, OWNER, None, None)
            )
            .await,
            before
        );
    }
}

#[tokio::test]
async fn message_feedback_reasons_refuse_reusing_a_request_id_with_new_content() {
    let (fixture, _, _) = feedback_fixture().await;
    json_response(
        &fixture.router,
        request(
            Method::PUT,
            AGENT_MESSAGE,
            OWNER,
            None,
            Some(json!({"vote":"down"})),
        ),
    )
    .await;
    let path = format!("{AGENT_MESSAGE}/reasons");
    for (version, reasons, expected) in [
        (1, json!(["inaccurate"]), StatusCode::OK),
        (2, json!(["other"]), StatusCode::CONFLICT),
    ] {
        let (status, body) = json_response(&fixture.router, request(Method::PUT, &path, OWNER, None, Some(json!({"expected_version":version,"request_id":"same-request","reasons":reasons})))).await;
        assert_eq!(status, expected, "{body}");
    }
}

#[tokio::test]
async fn message_feedback_reasons_reject_invalid_selection_and_request_identity() {
    let (fixture, _, _) = feedback_fixture().await;
    json_response(
        &fixture.router,
        request(
            Method::PUT,
            AGENT_MESSAGE,
            OWNER,
            None,
            Some(json!({"vote":"down"})),
        ),
    )
    .await;
    for (reasons, request_id) in [
        (json!(["inaccurate", "inaccurate"]), "duplicate"),
        (json!(["unknown"]), "unknown"),
        (json!([]), ""),
        (json!([]), "two words"),
    ] {
        let (status, body) = json_response(
            &fixture.router,
            request(
                Method::PUT,
                &format!("{AGENT_MESSAGE}/reasons"),
                OWNER,
                None,
                Some(json!({"expected_version":1,"request_id":request_id,"reasons":reasons})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
}

#[tokio::test]
async fn message_feedback_reasons_accept_500_unicode_codepoints_and_refuse_501() {
    let (fixture, _, _) = feedback_fixture().await;
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::PUT,
            AGENT_MESSAGE,
            OWNER,
            None,
            Some(json!({"vote":"down"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let path = format!("{AGENT_MESSAGE}/reasons");
    let request_body = json!({"expected_version":1,"request_id":"unicode-500", "reasons":["inaccurate", "other"],"comment":"🦀".repeat(500)});
    let (status, saved) = json_response(
        &fixture.router,
        request(Method::PUT, &path, OWNER, None, Some(request_body.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["comment"], "🦀".repeat(500));
    assert_eq!(saved["viewer_id"], OWNER);
    assert_eq!(saved["agent_id"], "agt_feedback");
    assert_eq!(saved["message_id"], "event:9");
    assert_eq!(saved["version"], 2);
    let (status, body) = json_response(&fixture.router,
        request(Method::PUT, &path, OWNER, None, Some(json!({
            "expected_version":2,"request_id":"unicode-501", "reasons":["other"],"comment":"🦀".repeat(501)
        })))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (_, current) = json_response(
        &fixture.router,
        request(Method::GET, AGENT_MESSAGE, OWNER, None, None),
    )
    .await;
    assert_eq!(current, saved);
    let (_, replay) = json_response(
        &fixture.router,
        request(Method::PUT, &path, OWNER, None, Some(request_body)),
    )
    .await;
    assert_eq!(replay, saved);
}
