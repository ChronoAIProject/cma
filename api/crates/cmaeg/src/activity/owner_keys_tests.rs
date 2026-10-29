use super::*;
use crate::{auth::owner_keys::tests::OwnerFixture, worker_credentials::WorkerHistory};

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
}
