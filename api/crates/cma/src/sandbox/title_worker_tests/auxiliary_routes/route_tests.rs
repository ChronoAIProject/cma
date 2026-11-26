use super::*;

#[tokio::test]
async fn both_workers_select_the_exact_scoped_service_and_deployment_model() {
    for (services, selected) in [
        (vec!["chrono-llm-public"], "chrono-llm-public"),
        (vec!["chrono-llm"], "chrono-llm"),
        (vec!["chrono-llm", "chrono-llm-public"], "chrono-llm-public"),
    ] {
        let mut sandbox = sandbox();
        sandbox.credential.reaches = services.into_iter().map(ServiceId::new).collect();
        let f = Fixture::with_records(sandbox, agent(), Capture::default(), None).await;
        f.turn(1, "Private title input");
        let peer = Peer::start(Reply::Success).await;
        assert_eq!(
            title_worker(&f, peer.endpoint())
                .run_one(&f.id)
                .await
                .unwrap(),
            AgentTitleOutcome::Generated
        );
        let work = suggestion_work(&f).await;
        assert_eq!(
            suggestion_worker(&f, peer.endpoint())
                .generate(&work)
                .await
                .unwrap()
                .output,
            "{\"suggestions\":[]}"
        );
        let calls = peer.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].uri,
            format!("/api/v1/proxy/s/{selected}/responses")
        );
        assert_eq!(
            calls[1].uri,
            format!("/api/v1/proxy/s/{selected}/v1/responses")
        );
        for call in calls.iter() {
            assert_eq!(call.headers["authorization"], format!("Bearer {KEY}"));
            assert!(!call.headers.contains_key("open-sandbox-api-key"));
            assert_eq!(call.body["model"], MODEL);
            assert_eq!(call.body["stream"], false);
            assert_eq!(call.body["store"], false);
            assert_eq!(call.body["max_output_tokens"], 1024);
        }
        assert_eq!(calls[0].body["input"][0]["content"], "Private title input");
        assert_eq!(calls[0].body["input"][1]["content"], "answer 1");
        assert_eq!(calls[1].headers["idempotency-key"], work.id);
        let context: serde_json::Value =
            serde_json::from_str(calls[1].body["input"][0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(context, serde_json::to_value(&work.context).unwrap());
        assert_eq!(calls[1].body["tools"], serde_json::json!([]));
        assert_eq!(calls[1].body["tool_choice"], "none");
        assert_no_legacy_proxy(&f);
    }
}

#[tokio::test]
async fn missing_model_grant_has_no_auxiliary_destination_or_title_redispatch() {
    let mut sandbox = sandbox();
    sandbox.credential.reaches = vec![ServiceId::new("github")];
    let f = Fixture::with_records(sandbox, agent(), Capture::default(), None).await;
    f.turn(1, "Unscoped title");
    let peer = Peer::start(Reply::Success).await;
    let title = title_worker(&f, peer.endpoint());
    assert_eq!(
        title.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Fallback
    );
    assert_eq!(
        title.run_one(&f.id).await.unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert!(matches!(
        suggestion_worker(&f, peer.endpoint())
            .generate(&suggestion_work(&f).await)
            .await,
        Err(SuggestionFailure::Unavailable)
    ));
    assert!(peer.calls.lock().unwrap().is_empty());
    assert_no_legacy_proxy(&f);
}

#[tokio::test]
async fn own_account_titles_require_separate_scope_while_suggestions_remain_refused() {
    for scoped in [false, true] {
        let mut sandbox = sandbox();
        sandbox.profile.content.model_access = ModelAccess::OwnCredentials;
        sandbox.credential.reaches = if scoped {
            vec![ServiceId::new("chrono-llm-public")]
        } else {
            vec![]
        };
        let f = Fixture::with_records(sandbox, agent(), Capture::default(), None).await;
        f.turn(1, "Own account task");
        let peer = Peer::start(Reply::Success).await;
        let outcome = title_worker(&f, peer.endpoint())
            .run_one(&f.id)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            if scoped {
                AgentTitleOutcome::Generated
            } else {
                AgentTitleOutcome::Fallback
            }
        );
        assert!(matches!(
            suggestion_worker(&f, peer.endpoint())
                .generate(&suggestion_work(&f).await)
                .await,
            Err(SuggestionFailure::Unavailable)
        ));
        assert_eq!(peer.calls.lock().unwrap().len(), usize::from(scoped));
        assert_no_legacy_proxy(&f);
    }
}
