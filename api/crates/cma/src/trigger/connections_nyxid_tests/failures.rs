use super::*;

#[tokio::test]
async fn provider_and_credential_failures_are_typed_and_sanitized() {
    let provider = Provider::start().await;
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_link_reply(Reply::error(
        StatusCode::NOT_FOUND,
        r#"{"message":"private provider detail"}"#,
    ));
    assert_eq!(
        broker(&provider, Tokens::available())
            .handoff_state(&owner, "link-one", &service)
            .await,
        Err(ServiceConnectionError::NotFound)
    );

    provider.state.set_create_reply(Reply::error(
        StatusCode::FORBIDDEN,
        r#"{"message":"private policy detail"}"#,
    ));
    assert_eq!(
        broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await,
        Err(ServiceConnectionError::Refused)
    );

    for status in [
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_EARLY,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        provider.state.set_create_reply(Reply::error(
            status,
            r#"{"message":"private retry detail"}"#,
        ));
        let error = broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await
            .expect_err("retryable provider failure is unavailable");
        assert_eq!(
            error,
            ServiceConnectionError::Unavailable("NyxID request failed".into())
        );
        assert!(!error.to_string().contains("private retry detail"));
    }

    provider.state.set_services_reply(Reply::error(
        StatusCode::SERVICE_UNAVAILABLE,
        r#"{"message":"database host and private detail"}"#,
    ));
    let unavailable = broker(&provider, Tokens::available())
        .requirements(&owner, std::slice::from_ref(&service))
        .await
        .expect_err("provider outage is unavailable");
    assert_eq!(
        unavailable,
        ServiceConnectionError::Unavailable("NyxID request failed".into())
    );
    assert!(!unavailable.to_string().contains("database host"));

    let credential_error = broker(
        &provider,
        Tokens::failing("encrypted store and private detail"),
    )
    .requirements(&owner, std::slice::from_ref(&service))
    .await
    .expect_err("credential outage is unavailable");
    assert_eq!(
        credential_error,
        ServiceConnectionError::Unavailable("visitor credential lookup failed".into())
    );
    assert!(!credential_error.to_string().contains("encrypted store"));

    assert_eq!(
        broker(&provider, Tokens::missing())
            .requirements(&owner, &[service])
            .await,
        Err(ServiceConnectionError::Refused)
    );
}

#[tokio::test]
async fn malformed_provider_payloads_fail_closed_without_echoing_the_body() {
    let provider = Provider::start().await;
    provider.state.set_create_reply(Reply::error(
        StatusCode::OK,
        r#"{"private_secret":"must-not-escape"}"#,
    ));

    let error = broker(&provider, Tokens::available())
        .create_handoff(
            &SubjectId::new("visitor-one"),
            &ServiceId::new("github"),
            "intent-one",
        )
        .await
        .expect_err("malformed response is refused");

    assert_eq!(
        error,
        ServiceConnectionError::Malformed("NyxID response was not usable".into())
    );
    assert!(!error.to_string().contains("must-not-escape"));
}

#[tokio::test]
async fn malformed_timestamps_and_provider_response_limits_fail_closed() {
    let provider = Provider::start().await;
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_create_reply(Reply::json(json!({
        "id": "link-one",
        "connect_url": trusted_connect_url(),
        "expires_at": "not-a-timestamp",
    })));
    assert_eq!(
        broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await,
        Err(malformed_error())
    );

    provider
        .state
        .set_create_reply(Reply::error(StatusCode::OK, &"x".repeat(1024 * 1024 + 1)));
    assert_eq!(
        broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await,
        Err(malformed_error())
    );

    let rows = (0..=1_000)
        .map(|index| {
            json!({
                "id": format!("service-{index}"),
                "slug": format!("service-{index}"),
                "is_active": true,
                "credential_source": { "type": "personal" }
            })
        })
        .collect::<Vec<_>>();
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": rows })));
    assert_eq!(
        broker(&provider, Tokens::available())
            .requirements(&owner, std::slice::from_ref(&service))
            .await,
        Err(malformed_error())
    );
}
