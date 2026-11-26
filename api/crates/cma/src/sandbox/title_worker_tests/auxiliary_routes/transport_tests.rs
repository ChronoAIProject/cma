use super::*;

#[tokio::test]
async fn redirects_never_disclose_auxiliary_credentials_or_content_to_another_peer() {
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        let alternate = Peer::start(Reply::Success).await;
        let selected = Peer::start(Reply::Redirect(
            status,
            format!("{}/v1/responses", alternate.origin),
        ))
        .await;
        let f = Fixture::new(Capture::default(), None).await;
        f.turn(1, "Private task");
        let title = title_worker(&f, selected.endpoint());
        assert_eq!(
            title.run_one(&f.id).await,
            Err(AgentTitleServiceError::GenerationUnavailable)
        );
        assert!(matches!(
            suggestion_worker(&f, selected.endpoint())
                .generate(&suggestion_work(&f).await)
                .await,
            Err(SuggestionFailure::Unavailable)
        ));
        assert_eq!(selected.calls.lock().unwrap().len(), 2);
        assert_eq!(
            title.run_one(&f.id).await.unwrap(),
            AgentTitleOutcome::Skipped
        );
        assert_eq!(selected.calls.lock().unwrap().len(), 2);
        assert!(alternate.calls.lock().unwrap().is_empty());
        assert_no_legacy_proxy(&f);
    }
}

#[tokio::test]
async fn provider_refusals_keep_the_selected_route_and_current_title_claim() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::PAYMENT_REQUIRED,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let selected = Peer::start(Reply::Failure(status)).await;
        let f = Fixture::new(Capture::default(), None).await;
        f.turn(1, "Private task");
        let title = title_worker(&f, selected.endpoint());
        let expected =
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::PAYMENT_REQUIRED {
                Ok(AgentTitleOutcome::Fallback)
            } else {
                Err(AgentTitleServiceError::GenerationUnavailable)
            };
        assert_eq!(title.run_one(&f.id).await, expected);
        assert_eq!(
            title.run_one(&f.id).await.unwrap(),
            AgentTitleOutcome::Skipped
        );
        assert!(matches!(
            suggestion_worker(&f, selected.endpoint())
                .generate(&suggestion_work(&f).await)
                .await,
            Err(SuggestionFailure::Unavailable)
        ));
        assert_eq!(selected.calls.lock().unwrap().len(), 2);
        assert_no_legacy_proxy(&f);
    }
}
