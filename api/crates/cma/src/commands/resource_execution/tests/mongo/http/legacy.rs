use super::{server::Server, *};
use layer2_core::resource_access::{ResourceShare, ResourceShareStore, ShareMode};

#[derive(Clone, Copy)]
enum Change {
    Revoke,
    SetView,
}

impl Change {
    fn request(
        self,
        fixture: &HttpFixture,
        server: &Server,
        cookie: &str,
        key: &str,
        revision: u64,
    ) -> reqwest::RequestBuilder {
        let layer2_core::resource_access::ResourceSelector::Sandbox { sandbox_id } =
            &fixture.data.selector
        else {
            panic!()
        };
        let path = format!(
            "/api/v1/resource-shares/sandbox/{sandbox_id}/{}",
            fixture.data.share.id
        );
        let request = match self {
            Self::Revoke => fixture.request(
                server,
                Method::DELETE,
                &format!("{path}?expected_revision={revision}"),
                Some(cookie),
            ),
            Self::SetView => fixture
                .request(server, Method::PATCH, &path, Some(cookie))
                .json(&json!({"mode":"view", "expected_revision":revision})),
        };
        request.header("Idempotency-Key", key)
    }
}

fn links(reply: &reqwest::Response, status: Option<&str>) {
    let actual: Vec<_> = reply
        .headers()
        .get_all("link")
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect();
    let discovery = crate::REST_LINKS;
    let expected: Vec<_> = status
        .into_iter()
        .chain(std::iter::once(discovery.to_str().unwrap()))
        .collect();
    assert_eq!(actual, expected);
}

async fn problem(
    request: reqwest::RequestBuilder,
    expected: StatusCode,
    code: &str,
    link: Option<&str>,
) {
    let reply = request.send().await.unwrap();
    assert_eq!(reply.status(), expected);
    assert_eq!(reply.headers()["content-type"], "application/problem+json");
    links(&reply, link);
    if link.is_some() {
        assert_eq!(reply.headers()["cache-control"], "no-store");
    }
    let body: Value = reply.json().await.unwrap();
    assert_eq!(body["code"], code);
    for field in [
        "operation_id",
        "share",
        "actor",
        "at",
        "policy_revision",
        "receipt",
        "participants",
        "unresolved_effects",
        "deadline_unix_ms",
    ] {
        assert!(
            body.get(field).is_none(),
            "unexpected receipt/status field {field}"
        );
    }
}

async fn share(fixture: &HttpFixture) -> ResourceShare {
    fixture
        .data
        .sandbox
        .resource_share(&fixture.data.share.id)
        .await
        .unwrap()
        .unwrap()
}

async fn retained(
    fixture: &HttpFixture,
    original: &RevocationOperationV1,
) -> RevocationOperationV1 {
    fixture
        .data
        .store
        .revocation(&original.key.operation_id)
        .await
        .unwrap()
        .unwrap()
}

async fn receipt(request: reqwest::RequestBuilder, observes: bool) -> Value {
    let reply = request.send().await.unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    assert_eq!(reply.headers()["content-type"], "application/json");
    links(&reply, None);
    let mut body: Value = reply.json().await.unwrap();
    let reference = body.as_object_mut().unwrap().remove("revocation");
    if observes {
        assert_eq!(
            reference.unwrap(),
            json!({
                "operation_id": body["operation_id"], "kind":"sandbox",
                "id": body["share"]["selector"]["sandbox_id"],
                "share_id": body["share"]["id"],
                "expected_revision": body["share"]["revision"].as_u64().unwrap() - 1,
            })
        );
    } else {
        assert!(reference.is_none());
    }
    let mut fields: Vec<_> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["actor", "at", "operation_id", "policy_revision", "share"]
    );
    body
}

async fn effective(
    fixture: &HttpFixture,
    original: &RevocationOperationV1,
    change: Change,
) -> RevocationOperationV1 {
    let stored = retained(fixture, original).await;
    assert_eq!(stored.key, original.key);
    assert!(stored.intent == original.intent);
    assert_eq!(stored.deadline_unix_ms, original.deadline_unix_ms);
    assert_eq!(stored.frozen_members, original.frozen_members);
    let RevocationPhaseV1::Effective { receipt, .. } = &stored.phase else {
        panic!("final participant evidence must produce an effective receipt")
    };
    assert_eq!(receipt.operation_id, original.key.operation_id.as_str());
    let mut expected = fixture.data.share.clone();
    expected.revision += 1;
    match change {
        Change::Revoke => {
            assert!(receipt.share.revoked_at.is_some());
            expected.revoked_at.clone_from(&receipt.share.revoked_at);
        }
        Change::SetView => expected.mode = ShareMode::View,
    }
    assert_eq!(receipt.share, expected);
    assert_eq!(share(fixture).await, expected);
    let snapshot = fixture
        .data
        .sandbox
        .resource_access_snapshot(&fixture.data.target)
        .await
        .unwrap();
    assert_eq!(snapshot.fence().policy_revision, receipt.policy_revision);
    stored
}

// Real HTTP/cookie middleware and activated Mongo; participant endpoints remain synthetic.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_share_changes_preserve_pending_identity_until_effective_receipt() {
    for change in [Change::Revoke, Change::SetView] {
        let fixture = HttpFixture::new().await;
        let row = fixture.data.register("legacy-http").await;
        let mut first = fixture.server(false, false).await;
        let request = |server: &Server, cookie: &str, key: &str| {
            change.request(&fixture, server, cookie, key, fixture.data.share.revision)
        };
        let reply = request(&first, &fixture.owner, "legacy-original")
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::CONFLICT);
        let original = operation(&fixture).await;
        assert!(matches!(original.phase, RevocationPhaseV1::Pending));
        assert_eq!(original.frozen_members, 1);
        let path = fixture.status_path(original.key.operation_id.as_str());
        let link = format!("<{path}>; rel=\"status\"");
        links(&reply, Some(&link));
        assert_eq!(reply.headers()["cache-control"], "no-store");
        assert_eq!(reply.headers()["content-type"], "application/problem+json");
        let pending: Value = reply.json().await.unwrap();
        assert_eq!(pending["code"], "resource_revocation_pending");
        assert!(pending.get("receipt").is_none());
        assert!(pending.get("share").is_none());
        assert_eq!(share(&fixture).await, fixture.data.share);
        first.stop().await;
        let mut replacement = fixture.server(false, true).await;
        let pending_status = response(
            fixture.request(&replacement, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::OK,
        )
        .await;
        assert_eq!(pending_status["state"], "pending");
        assert_eq!(
            pending_status["operation_id"],
            original.key.operation_id.as_str()
        );
        assert_eq!(
            pending_status["deadline_unix_ms"],
            original.deadline_unix_ms
        );
        assert!(retained(&fixture, &original).await == original);
        problem(
            request(&replacement, &fixture.owner, "legacy-original"),
            StatusCode::CONFLICT,
            "resource_revocation_pending",
            Some(&link),
        )
        .await;
        problem(
            request(&replacement, &fixture.owner, "legacy-replacement"),
            StatusCode::CONFLICT,
            "resource_revocation_conflict",
            None,
        )
        .await;
        problem(
            request(&replacement, &fixture.other, "legacy-original"),
            StatusCode::NOT_FOUND,
            "resource_not_found",
            None,
        )
        .await;
        assert!(operation(&fixture).await == original);
        assert_eq!(share(&fixture).await, fixture.data.share);

        let worker = RevocationWorker::new(fixture.data.reopened().await, Directory::new(&[row]));
        assert_eq!(
            worker.reconcile(&original.key.operation_id).await.unwrap(),
            Progress::Effective
        );
        let final_state = effective(&fixture, &original, change).await;
        let status = response(
            fixture.request(&replacement, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::OK,
        )
        .await;
        assert_eq!(status["state"], "effective");
        let result = receipt(
            request(&replacement, &fixture.owner, "legacy-original"),
            true,
        )
        .await;
        assert_eq!(result, status["receipt"]);
        assert_eq!(result["operation_id"], original.key.operation_id.as_str());
        assert_eq!(
            result,
            receipt(
                request(&replacement, &fixture.owner, "legacy-original"),
                true
            )
            .await
        );
        problem(
            request(&replacement, &fixture.other, "legacy-original"),
            StatusCode::NOT_FOUND,
            "resource_not_found",
            None,
        )
        .await;
        assert!(retained(&fixture, &original).await == final_state);
        assert!(fixture
            .data
            .store
            .pending_revocations(None, 32)
            .await
            .unwrap()
            .is_empty());
        replacement.stop().await;
        fixture.cleanup().await;
    }
}

// Current router/store with its optional service omitted, not an older binary qualification.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn omitted_revocation_service_cannot_bypass_activation_but_replays_final_receipts() {
    for change in [Change::Revoke, Change::SetView] {
        let fixture = HttpFixture::new().await;
        let row = fixture.data.register("omitted-service").await;
        let mut omitted = fixture.without_revocations(false).await;
        let request = |server: &Server, key: &str, revision: u64| {
            change.request(&fixture, server, &fixture.owner, key, revision)
        };
        problem(
            request(&omitted, "omitted-original", 1),
            StatusCode::CONFLICT,
            "resource_access_changed",
            None,
        )
        .await;
        assert_eq!(share(&fixture).await, fixture.data.share);
        assert!(fixture
            .data
            .store
            .pending_revocations(None, 32)
            .await
            .unwrap()
            .is_empty());
        problem(
            fixture.create(&omitted, Some(&fixture.owner), "unavailable-create"),
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_revocation_unavailable",
            None,
        )
        .await;

        let mut coordinated = fixture.server(false, true).await;
        let reply = request(&coordinated, "omitted-original", 1)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::CONFLICT);
        let original = operation(&fixture).await;
        let path = fixture.status_path(original.key.operation_id.as_str());
        let link = format!("<{path}>; rel=\"status\"");
        links(&reply, Some(&link));
        let pending: Value = reply.json().await.unwrap();
        assert_eq!(pending["code"], "resource_revocation_pending");
        coordinated.stop().await;
        problem(
            fixture.request(&omitted, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_revocation_unavailable",
            None,
        )
        .await;
        for key in ["omitted-original", "bypass-pending"] {
            problem(
                request(&omitted, key, 1),
                StatusCode::CONFLICT,
                "resource_access_changed",
                None,
            )
            .await;
        }
        assert_eq!(share(&fixture).await, fixture.data.share);
        assert!(operation(&fixture).await == original);
        let mut restored = fixture.server(false, false).await;
        problem(
            request(&restored, "omitted-original", 1),
            StatusCode::CONFLICT,
            "resource_revocation_pending",
            Some(&link),
        )
        .await;
        assert!(operation(&fixture).await == original);

        let worker = RevocationWorker::new(fixture.data.reopened().await, Directory::new(&[row]));
        assert_eq!(
            worker.reconcile(&original.key.operation_id).await.unwrap(),
            Progress::Effective
        );
        let final_state = effective(&fixture, &original, change).await;
        let status = response(
            fixture.request(&restored, Method::GET, &path, Some(&fixture.owner)),
            StatusCode::OK,
        )
        .await;
        assert_eq!(status["state"], "effective");
        for _ in 0..2 {
            assert_eq!(
                receipt(request(&omitted, "omitted-original", 1), false).await,
                status["receipt"]
            );
        }
        problem(
            request(&omitted, "bypass-effective", 2),
            StatusCode::CONFLICT,
            "resource_access_changed",
            None,
        )
        .await;
        assert!(retained(&fixture, &original).await == final_state);
        assert!(fixture
            .data
            .store
            .pending_revocations(None, 32)
            .await
            .unwrap()
            .is_empty());
        effective(&fixture, &original, change).await;
        omitted.stop().await;
        restored.stop().await;
        fixture.cleanup().await;
    }
}
