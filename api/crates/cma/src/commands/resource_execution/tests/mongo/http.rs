use super::*;
use crate::auth::persistence::{AuthStore, SecretDigest};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

mod fixture;
mod legacy;
mod oauth;
mod provider;
mod server;
use fixture::HttpFixture;

async fn response(request: reqwest::RequestBuilder, expected: StatusCode) -> Value {
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), expected);
    if expected.is_success() {
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    response.json().await.unwrap()
}

async fn operation(fixture: &HttpFixture) -> RevocationOperationV1 {
    let pending = fixture
        .data
        .store
        .pending_revocations(None, 32)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    pending.into_iter().next().unwrap()
}

async fn refuse(
    request: reqwest::RequestBuilder,
    expected: StatusCode,
    operation: &RevocationOperationV1,
) {
    let value = response(request, expected).await;
    assert!(!value
        .to_string()
        .contains(operation.key.operation_id.as_str()));
    assert!(!value
        .to_string()
        .contains(operation.key.family.share_id.as_str()));
    for field in [
        "operation_id",
        "receipt",
        "participants",
        "unresolved_effects",
    ] {
        assert!(value.get(field).is_none(), "private status field {field}");
    }
}

// Real HTTP/cookie middleware and Mongo custody; synthetic identity and stop endpoints.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn http_cookie_reply_loss_recovers_original_revocation_across_services() {
    let fixture = HttpFixture::new().await;
    let row = fixture.data.register("http-recovery").await;
    let mut first = fixture.server(true, false).await;
    if let Ok(reply) = fixture
        .create(&first, Some(&fixture.owner), "original-http-key")
        .send()
        .await
    {
        assert!(
            reply.bytes().await.is_err(),
            "the admitted response must be lost on the wire"
        );
    }
    let original = operation(&fixture).await;
    assert!(matches!(original.phase, RevocationPhaseV1::Pending));
    first.stop().await;
    let mut replacement = fixture.server(false, true).await;
    let pending = response(
        fixture.create(&replacement, Some(&fixture.owner), "original-http-key"),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(pending["operation_id"], original.key.operation_id.as_str());
    assert_eq!(pending["deadline_unix_ms"], original.deadline_unix_ms);
    assert_eq!(pending["state"], "pending");
    assert!(pending["receipt"].is_null());
    response(
        fixture.create(&replacement, Some(&fixture.owner), "replacement-http-key"),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(operation(&fixture).await == original);
    let worker = RevocationWorker::new(fixture.data.reopened().await, Directory::new(&[row]));
    assert_eq!(
        worker.reconcile(&original.key.operation_id).await.unwrap(),
        Progress::Effective
    );
    let final_state = final_operation(&fixture.data, &original).await;
    let path = fixture.status_path(original.key.operation_id.as_str());
    let effective = response(
        fixture.request(&replacement, Method::GET, &path, Some(&fixture.owner)),
        StatusCode::OK,
    )
    .await;
    assert_eq!(effective["state"], "effective");
    assert_eq!(effective["operation_id"], pending["operation_id"]);
    assert_eq!(effective["deadline_unix_ms"], pending["deadline_unix_ms"]);
    assert!(effective["receipt"].is_object());
    let replay = response(
        fixture.create(&replacement, Some(&fixture.owner), "original-http-key"),
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(replay, effective);
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
    assert!(fixture.provider.calls() >= 8);
    replacement.stop().await;
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn http_cookie_revocation_status_requires_live_owner_custody() {
    let fixture = HttpFixture::new().await;
    let row = fixture.data.register("http-privacy").await;
    let mut first = fixture.server(false, false).await;
    response(
        fixture.create(&first, Some(&fixture.owner), "private-http-key"),
        StatusCode::ACCEPTED,
    )
    .await;
    let original = operation(&fixture).await;
    let path = fixture.status_path(original.key.operation_id.as_str());
    let expired = fixture.cookie("owner", true).await;
    let revoked = fixture.cookie("owner", false).await;
    fixture
        .custody
        .delete_browser_login(&SecretDigest::identify("browser-login", &revoked))
        .await
        .unwrap();
    let mut second = fixture.server(false, true).await;
    for server in [&first, &second] {
        for cookie in [
            None,
            Some("forged-cookie"),
            Some(expired.as_str()),
            Some(revoked.as_str()),
        ] {
            refuse(
                fixture.request(server, Method::GET, &path, cookie),
                StatusCode::UNAUTHORIZED,
                &original,
            )
            .await;
            refuse(
                fixture.create(server, cookie, "private-http-key"),
                StatusCode::UNAUTHORIZED,
                &original,
            )
            .await;
        }
        refuse(
            fixture.request(server, Method::GET, &path, Some(&fixture.other)),
            StatusCode::NOT_FOUND,
            &original,
        )
        .await;
        refuse(
            fixture.create(server, Some(&fixture.other), "private-http-key"),
            StatusCode::NOT_FOUND,
            &original,
        )
        .await;
        let pending = response(
            fixture.request(server, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::OK,
        )
        .await;
        assert_eq!(pending["state"], "pending");
    }
    assert!(operation(&fixture).await == original);
    let before = fixture.provider.calls();
    fixture.provider.authority("owner", false, true).await;
    refuse(
        fixture.request(&second, Method::GET, &path, Some(&fixture.owner)),
        StatusCode::UNAUTHORIZED,
        &original,
    )
    .await;
    assert!(fixture.provider.calls() > before);
    fixture.provider.authority("owner", true, false).await;
    refuse(
        fixture.request(&second, Method::GET, &path, Some(&fixture.owner)),
        StatusCode::NOT_FOUND,
        &original,
    )
    .await;
    fixture.provider.authority("owner", true, true).await;
    let worker = RevocationWorker::new(fixture.data.reopened().await, Directory::new(&[row]));
    assert_eq!(
        worker.reconcile(&original.key.operation_id).await.unwrap(),
        Progress::Effective
    );
    let final_state = final_operation(&fixture.data, &original).await;
    for server in [&first, &second] {
        let effective = response(
            fixture.request(server, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::OK,
        )
        .await;
        assert_eq!(effective["state"], "effective");
        refuse(
            fixture.request(server, Method::GET, &path, Some(&fixture.other)),
            StatusCode::NOT_FOUND,
            &original,
        )
        .await;
    }
    fixture
        .custody
        .delete_browser_login(&SecretDigest::identify("browser-login", &fixture.owner))
        .await
        .unwrap();
    for server in [&first, &second] {
        refuse(
            fixture.request(server, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::UNAUTHORIZED,
            &original,
        )
        .await;
    }
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
    first.stop().await;
    second.stop().await;
    fixture.cleanup().await;
}
