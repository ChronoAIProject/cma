use super::*;
use crate::{auth::owner_keys::tests::OwnerFixture, worker_credentials::WorkerHistory};

#[tokio::test]
async fn scoped_owner_key_reads_public_management_routes_and_refuses_foreign_scope_and_owner() {
    let (server, mut state, mut request, owner) = fixture_app().await;
    super::privacy::private_actor(&mut request);
    let channel = Arc::make_mut(&mut request.channel);
    channel.definition.creation = Some(cmaeg_core::channel::ChannelCreation {
        channel_id: channel.id.clone(),
        fingerprint: "a".repeat(64),
        ready: true,
    });
    server
        .memory
        .channels
        .lock()
        .unwrap()
        .get_mut(channel.definition.record_id.as_str())
        .unwrap()
        .blob = seal_blob(
        &fixture::keys(),
        &serde_json::to_vec(&channel.definition.to_blob_json()).unwrap(),
    )
    .unwrap();
    let auth = OwnerFixture::start(&owner.subject).await;
    state.creator_auth = Arc::new(auth.auth.clone());
    state.activity.received(&request).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, crate::router_with_state(state))
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let paths = [
        format!(
            "/v1/channel-records/{}",
            request.channel.definition.record_id
        ),
        format!("/v1/channels/{}", request.channel.id),
        format!("/v1/channels/{}/activity", request.channel.id),
        format!(
            "/v1/channels/{}/activity?diagnostics=true&raw=true",
            request.channel.id
        ),
        format!("/v1/channels/{}/activity/stream", request.channel.id),
    ];
    let original = auth.claims.lock().unwrap().clone();
    for path in &paths {
        let response = client
            .get(format!("{base}{path}"))
            .headers(auth.headers.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store");
        if path.contains("/activity") {
            let bytes = response.bytes_stream().next().await.unwrap().unwrap();
            let body = std::str::from_utf8(&bytes).unwrap();
            assert!(body.contains("Participant"));
            assert!(body.contains("redacted"));
            assert!(!body.contains(&request.activity.actor.id));
            assert!(!body.contains(request.activity.actor.display_name.as_ref().unwrap()));
        }
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .headers(auth.headers.clone())
                .header("X-CMAEG-Owner", "foreign-organization")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for (claim, value, status) in [
        (
            "sub",
            serde_json::json!("another-owner"),
            StatusCode::NOT_FOUND,
        ),
        (
            "aud",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
            StatusCode::UNAUTHORIZED,
        ),
        ("active", serde_json::json!(false), StatusCode::UNAUTHORIZED),
        ("permissions", serde_json::json!([]), StatusCode::FORBIDDEN),
    ] {
        *auth.claims.lock().unwrap() = original.clone();
        auth.claims.lock().unwrap()[claim] = value;
        for path in &paths {
            let response = client
                .get(format!("{base}{path}"))
                .headers(auth.headers.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{claim} {path}");
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
            let body = response.text().await.unwrap();
            assert!(!body.contains(request.channel.definition.record_id.as_str()));
            assert!(!body.contains(&owner.subject));
            assert!(!body.contains(&auth.token));
        }
    }
    *auth.claims.lock().unwrap() = original;
    let response = client
        .delete(format!("{base}{}", paths[1]))
        .headers(auth.headers.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = client
        .get(format!("{base}{}", paths[1]))
        .headers(auth.headers.clone())
        .header("X-CMAEG-Owner", "foreign-organization")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = client
        .get(format!("{base}{}", paths[1]))
        .headers(auth.headers.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    task.abort();
    auth.assert_introspection_requests();
}

#[tokio::test]
async fn owner_service_key_stream_closes_on_demotion_revocation_or_permission_removal() {
    for refusal in ["worker", "manager", "revoked", "permission"] {
        let (_server, mut state, request, old_creator) = fixture_app().await;
        let auth = OwnerFixture::start(&old_creator.subject).await;
        state.creator_auth = Arc::new(auth.auth.clone());
        let creator = auth.auth.authenticate(&auth.headers).await.unwrap();
        state.activity.received(&request).await.unwrap();
        let response = crate::http::activity::stream(
            State(state.clone()),
            creator,
            Path(request.channel.id.to_string()),
            Query(Default::default()),
            auth.headers.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        assert!(
            String::from_utf8_lossy(&stream.next().await.unwrap().unwrap())
                .contains("event: activity")
        );
        match refusal {
            "worker" => auth
                .history
                .record(&crate::channels::definition::hash_secret(&auth.token))
                .await
                .unwrap(),
            "manager" => auth.manager.known.store(true, Ordering::SeqCst),
            "revoked" => auth.claims.lock().unwrap()["active"] = serde_json::json!(false),
            "permission" => auth.claims.lock().unwrap()["permissions"] = serde_json::json!([]),
            _ => unreachable!(),
        }
        state
            .activity
            .change(
                request.channel.definition.record_id.as_str(),
                request.activity.event_id.as_str(),
                &Change::state("default", ConsumerState::Accepted, None),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(frame) = stream.next().await {
                assert!(
                    !String::from_utf8_lossy(&frame.unwrap()).contains("event: activity"),
                    "emitted after {refusal}"
                );
            }
        })
        .await
        .expect("owner key stream did not close promptly");
        auth.assert_introspection_requests();
    }
}

#[tokio::test]
async fn verified_personal_key_still_cannot_read_another_owners_channel() {
    let (_server, mut state, request, _) = fixture_app().await;
    let auth = OwnerFixture::start("different-owner").await;
    state.creator_auth = Arc::new(auth.auth.clone());
    let creator = auth.auth.authenticate(&auth.headers).await.unwrap();
    let response =
        crate::http::activity::latest(State(state), creator, Path(request.channel.id.to_string()))
            .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    auth.assert_introspection_requests();
}
