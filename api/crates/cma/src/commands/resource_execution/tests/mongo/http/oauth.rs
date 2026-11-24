use super::{server::Server, *};
use layer2_core::SubjectId;

mod fixture;
use fixture::{begin, callback, complete, session};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn oauth_callback_and_revocation_recover_across_http_service_replacement() {
    let fixture = HttpFixture::unseeded().await;
    assert!(fixture
        .custody
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .is_none());
    let row = fixture.data.register("oauth-recovery").await;
    let mut first = fixture.server(false, false).await;
    let attempt = begin(&fixture, &first, "owner").await;
    assert!(fixture
        .custody
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .is_none());
    assert_eq!(fixture.provider.exchanges(), 0);
    first.stop().await;

    let mut second = fixture.server(false, true).await;
    let owner = complete(&fixture, &second, &attempt, "owner").await;
    assert_eq!(fixture.provider.exchanges(), 1);
    let replay = callback(&fixture, &second, &attempt).send().await.unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    assert!(session(&replay).is_none());
    assert_eq!(fixture.provider.exchanges(), 1);
    let pending = response(
        fixture.create(&second, Some(&owner), "oauth-recovery-key"),
        StatusCode::ACCEPTED,
    )
    .await;
    let original = operation(&fixture).await;
    assert_eq!(pending["operation_id"], original.key.operation_id.as_str());
    assert_eq!(pending["state"], "pending");
    second.stop().await;

    let mut replacement = fixture.server(false, false).await;
    let path = fixture.status_path(original.key.operation_id.as_str());
    let replay = response(
        fixture.create(&replacement, Some(&owner), "oauth-recovery-key"),
        StatusCode::ACCEPTED,
    )
    .await;
    for field in [
        "operation_id",
        "deadline_unix_ms",
        "state",
        "receipt",
        "participants",
    ] {
        assert_eq!(replay[field], pending[field], "{field}");
    }
    let other_attempt = begin(&fixture, &replacement, "other-owner").await;
    let other = complete(&fixture, &replacement, &other_attempt, "other-owner").await;
    refuse(
        fixture.request(&replacement, Method::GET, &path, Some(&other)),
        StatusCode::NOT_FOUND,
        &original,
    )
    .await;
    refuse(
        fixture.create(&replacement, Some(&other), "oauth-recovery-key"),
        StatusCode::NOT_FOUND,
        &original,
    )
    .await;
    assert!(operation(&fixture).await == original);
    let worker = RevocationWorker::new(fixture.data.reopened().await, Directory::new(&[row]));
    assert_eq!(
        worker.reconcile(&original.key.operation_id).await.unwrap(),
        Progress::Effective
    );
    let final_state = final_operation(&fixture.data, &original).await;
    let effective = response(
        fixture.request(&replacement, Method::GET, &path, Some(&owner)),
        StatusCode::OK,
    )
    .await;
    assert_eq!(effective["state"], "effective");
    assert_eq!(effective["operation_id"], pending["operation_id"]);
    assert_eq!(effective["deadline_unix_ms"], pending["deadline_unix_ms"]);
    refuse(
        fixture.request(&replacement, Method::GET, &path, Some(&other)),
        StatusCode::NOT_FOUND,
        &original,
    )
    .await;
    let logout = fixture
        .request(&replacement, Method::GET, "/auth/logout", Some(&owner))
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::SEE_OTHER);
    let replay = callback(&fixture, &replacement, &attempt)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
    assert!(session(&replay).is_none());
    assert_eq!(fixture.provider.exchanges(), 2);
    refuse(
        fixture.request(&replacement, Method::GET, &path, Some(&owner)),
        StatusCode::UNAUTHORIZED,
        &original,
    )
    .await;
    assert!(
        fixture
            .data
            .store
            .revocation(&original.key.operation_id)
            .await
            .unwrap()
            .unwrap()
            == final_state
    );
    replacement.stop().await;
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn oauth_callback_refusals_never_issue_revocation_management_authority() {
    for joined in [false, true] {
        let fixture = HttpFixture::unseeded().await;
        let mut server = fixture.server(false, joined).await;
        let attempt = begin(&fixture, &server, "owner").await;
        let missing = fixture
            .request(&server, Method::GET, &attempt.path, None)
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
        assert!(session(&missing).is_none());
        let wrong = fixture
            .request(&server, Method::GET, &attempt.path, None)
            .header(
                "cookie",
                format!(
                    "{}=wrong-binding",
                    attempt.binding.split_once('=').unwrap().0
                ),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
        assert!(session(&wrong).is_none());
        assert_eq!(fixture.provider.exchanges(), 0);

        fixture.provider.authority("owner", false, true).await;
        let inactive = callback(&fixture, &server, &attempt).send().await.unwrap();
        assert_eq!(inactive.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(session(&inactive).is_none());
        assert_eq!(fixture.provider.exchanges(), 1);
        assert!(fixture
            .custody
            .subject(&SubjectId::new("owner"))
            .await
            .unwrap()
            .is_none());
        let binding = attempt.binding.split_once('=').unwrap().1;
        assert!(fixture
            .custody
            .browser_login(&SecretDigest::identify("browser-login", binding))
            .await
            .unwrap()
            .is_none());
        let denied = fixture
            .create(&server, Some(binding), "refused-oauth-key")
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert!(fixture
            .data
            .store
            .pending_revocations(None, 32)
            .await
            .unwrap()
            .is_empty());

        fixture.provider.authority("owner", true, true).await;
        let replay = callback(&fixture, &server, &attempt).send().await.unwrap();
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
        assert!(session(&replay).is_none());
        assert_eq!(fixture.provider.exchanges(), 1);
        let fresh = begin(&fixture, &server, "owner").await;
        let owner = complete(&fixture, &server, &fresh, "owner").await;
        let accepted = response(
            fixture.create(&server, Some(&owner), "refused-oauth-key"),
            StatusCode::ACCEPTED,
        )
        .await;
        assert_eq!(accepted["state"], "evidence_received");
        assert!(accepted["receipt"].is_null());
        assert_eq!(operation(&fixture).await.frozen_members, 0);
        server.stop().await;
        fixture.cleanup().await;
    }
}
