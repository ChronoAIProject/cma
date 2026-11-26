use super::{failure_tests, maintenance::StorageMaintenance, *};
use crate::dependency_test_support::{assert_deadline, finish, Peer};
use layer2_core::document_artifacts::{
    BlobDeletion, BlobMaintenanceError, DocumentBlobMaintenance,
};
use serde_json::json;

fn config(peer: &Peer) -> DocumentStorageConfig {
    DocumentStorageConfig {
        resilience_enabled: false,
        platform_origin: peer.origin.clone(),
        bucket: "fixture".into(),
        client_id: "fixture-machine".into(),
        client_secret: "local-fixture".to_owned().into(),
    }
}
pub(super) fn client(peer: &Peer, enabled: bool) -> Arc<StorageMaintenance> {
    StorageMaintenance::new(config(peer), enabled).unwrap()
}
pub(super) fn prefix() -> String {
    format!("documents/{}/", "a".repeat(64))
}
pub(super) fn operation(
    client: &Arc<StorageMaintenance>,
    list: bool,
) -> tokio::task::JoinHandle<Result<(), BlobMaintenanceError>> {
    let client = client.clone();
    tokio::spawn(async move {
        if list {
            client.list(&prefix(), None).await.map(|_| ())
        } else {
            client.delete(&failure_tests::key()).await.map(|_| ())
        }
    })
}
pub(super) async fn authorize(peer: &mut Peer) {
    let request = peer.next().await;
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.uri.path(), "/oauth/token");
    let fields: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(&request.body)
        .into_owned()
        .collect();
    assert_eq!(
        fields["scope"],
        "proxy object:read object:list object:delete"
    );
    request.json(json!({"access_token":"maintenance-fixture","expires_in":3600}));
}

#[tokio::test]
async fn maintenance_token_requests_maintenance_scope_and_online_token_keeps_original_scope() {
    let mut peer = Peer::start().await;
    let maintenance = client(&peer, true);
    let online = failure_tests::client(&peer);
    let task = operation(&maintenance, false);
    authorize(&mut peer).await;
    let request = peer.next().await;
    assert_eq!(request.method, Method::DELETE);
    assert_eq!(
        request.headers["authorization"],
        "Bearer maintenance-fixture"
    );
    assert_eq!(
        request.uri.path(),
        "/api/v1/proxy/s/chrono-storage-service/api/buckets/fixture/objects"
    );
    let query: Vec<_> = url::form_urlencoded::parse(request.uri.query().unwrap().as_bytes())
        .into_owned()
        .collect();
    assert_eq!(query, [("key".into(), failure_tests::key())]);
    request.respond(204, "");
    finish(task).await.unwrap();
    let task = failure_tests::operation(&online, false);
    failure_tests::authorize(&mut peer).await;
    peer.next().await.respond(200, "fixture");
    finish(task).await.unwrap();
    assert_eq!(peer.count(), 4);
}

#[tokio::test]
async fn deletion_disabled_never_contacts_storage() {
    let peer = Peer::start().await;
    for resilience_enabled in [false, true] {
        let mut settings = config(&peer);
        settings.resilience_enabled = resilience_enabled;
        let client = StorageMaintenance::new(settings, false).unwrap();
        assert!(!client.deletion_supported());
        for list in [false, true] {
            assert_eq!(
                finish(operation(&client, list)).await.unwrap_err().error,
                DocumentError::Unavailable
            );
        }
    }
    assert_eq!(peer.count(), 0);
}

#[tokio::test]
async fn delete_absent_key_is_success() {
    let mut peer = Peer::start().await;
    let client = client(&peer, true);
    assert!(client.deletion_supported());
    for (status, expected) in [
        (200, BlobDeletion::Deleted),
        (204, BlobDeletion::Deleted),
        (404, BlobDeletion::Absent),
    ] {
        let client = client.clone();
        let task = tokio::spawn(async move { client.delete(&failure_tests::key()).await });
        if peer.count() == 0 {
            authorize(&mut peer).await;
        }
        peer.next().await.respond(status, "");
        assert_eq!(finish(task).await.unwrap(), expected);
    }
    assert_eq!(peer.count(), 4);
}

#[tokio::test]
async fn fenced_deletion_requires_confirmation_for_the_exact_key_and_generation() {
    let mut peer = Peer::start().await;
    let client = client(&peer, true);
    let key = failure_tests::key();
    for (index, status) in [404, 200, 200, 200].into_iter().enumerate() {
        let namespace = client.namespace_sha256();
        let client = client.clone();
        let request_key = key.clone();
        let task = tokio::spawn(async move { client.delete_fenced(&request_key, 3).await });
        if index == 0 {
            authorize(&mut peer).await;
        }
        let request = peer.next().await;
        assert_eq!(request.method, Method::DELETE);
        assert!(request.uri.path().ends_with("/objects/fenced-v2"));
        let query: std::collections::BTreeMap<_, _> =
            url::form_urlencoded::parse(request.uri.query().unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(query["key"], key);
        assert_eq!(query["mutationGeneration"], "3");
        if status == 404 {
            request.respond(status, "");
            assert_eq!(
                finish(task).await.unwrap_err().error,
                DocumentError::Unavailable
            );
        } else {
            let generation = if index == 1 { 2 } else { 3 };
            request.json(json!({ "data": { "deleted": true, "generation": generation,
                "fenceVersion": if index == 2 { 1 } else { 2 }, "key": key }, "error": null }));
            if index == 1 || index == 2 {
                assert_eq!(
                    finish(task).await.unwrap_err().error,
                    DocumentError::Corrupt
                );
            } else {
                assert!(finish(task).await.unwrap().matches(&key, 3, &namespace));
            }
        }
    }
}

#[tokio::test]
async fn delete_treats_only_200_and_204_as_deleted() {
    let mut peer = Peer::start().await;
    let client = client(&peer, true);
    for (index, status) in [
        200, 204, 404, 202, 201, 203, 205, 206, 207, 208, 226, 299, 301, 400, 429, 500,
    ]
    .into_iter()
    .enumerate()
    {
        let copy = client.clone();
        let task = tokio::spawn(async move { copy.delete(&failure_tests::key()).await });
        if index == 0 {
            authorize(&mut peer).await;
        }
        peer.next().await.respond(status, "");
        let result = finish(task).await;
        match status {
            200 | 204 => assert_eq!(result.unwrap(), BlobDeletion::Deleted),
            404 => assert_eq!(result.unwrap(), BlobDeletion::Absent),
            _ => assert_eq!(result.unwrap_err().error, DocumentError::Unavailable),
        }
        assert_eq!(peer.count(), index + 2);
    }
}

#[tokio::test]
async fn delete_and_list_reject_invalid_or_foreign_prefix_without_contacting_storage() {
    let peer = Peer::start().await;
    let client = client(&peer, true);
    let owner = "a".repeat(64);
    for bad in [
        "https://other.invalid/secret".into(),
        "../other".into(),
        "cma/checkpoints/".into(),
        "documents/".into(),
        format!("documents/{owner}"),
        format!("documents/{owner}/../"),
        format!("documents/{owner}/doc_bad/"),
        format!("documents/{owner}/doc_{}/extra/", "b".repeat(32)),
        format!("documents/{}/", "g".repeat(64)),
        format!("documents/{owner}/%2f/"),
        failure_tests::key(),
    ] {
        assert_eq!(
            client.list(&bad, None).await.unwrap_err().error,
            DocumentError::Invalid
        );
    }
    for bad in [
        "https://other.invalid/secret",
        "cma/checkpoints/a",
        "documents/owner/../../secret",
        &prefix(),
    ] {
        assert_eq!(
            client.delete(bad).await.unwrap_err().error,
            DocumentError::Invalid
        );
    }
    assert_eq!(
        client
            .list(&prefix(), Some(&"x".repeat(4097)))
            .await
            .unwrap_err()
            .error,
        DocumentError::Invalid
    );
    assert_eq!(peer.count(), 0);
}

#[tokio::test]
async fn list_pages_are_bounded_and_cursor_is_opaque() {
    let mut peer = Peer::start().await;
    let client = client(&peer, true);
    let opaque = "https://other.invalid/?cursor=a+b&next=%2F#tail";
    for selected in [prefix(), format!("{}doc_{}/", prefix(), "b".repeat(32))] {
        let copy = client.clone();
        let requested = selected.clone();
        let task = tokio::spawn(async move { copy.list(&requested, Some(opaque)).await });
        if peer.count() == 0 {
            authorize(&mut peer).await;
        }
        let request = peer.next().await;
        assert_eq!(request.method, Method::GET);
        assert_eq!(
            request.uri.path(),
            "/api/v1/proxy/s/chrono-storage-service/api/buckets/fixture/objects"
        );
        let query: std::collections::BTreeMap<_, _> =
            url::form_urlencoded::parse(request.uri.query().unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(query["prefix"], selected);
        assert_eq!(query["cursor"], opaque);
        assert_eq!(query["limit"], "256");
        request.json(json!({"data":{"keys":[failure_tests::key()],"next":opaque}}));
        let page = finish(task).await.unwrap();
        assert_eq!(page.keys, [failure_tests::key()]);
        assert_eq!(page.next.as_deref(), Some(opaque));
    }
    let task = operation(&client, true);
    peer.next().await.respond(200, "x".repeat(256 * 1024 + 1));
    assert_eq!(
        finish(task).await.unwrap_err().error,
        DocumentError::Corrupt
    );
    assert_eq!(peer.count(), 4);
}

#[tokio::test]
async fn list_rejects_malformed_excessive_or_foreign_rows() {
    let mut peer = Peer::start().await;
    let client = client(&peer, true);
    for response in [
        json!({"keys":[]}),
        json!({"data":{"keys":["https://other.invalid/"]}}),
        json!({"data":{"keys":[failure_tests::key().replace(&"a".repeat(64), &"c".repeat(64))]}}),
        json!({"data":{"keys":vec![failure_tests::key();257]}}),
        json!({"data":{"keys":[],"next":"x".repeat(4097)}}),
        json!({"data":{"keys":[]},"error":{"message":"refused"}}),
    ] {
        let task = operation(&client, true);
        if peer.count() == 0 {
            authorize(&mut peer).await;
        }
        peer.next().await.json(response);
        assert_eq!(
            finish(task).await.unwrap_err().error,
            DocumentError::Corrupt
        );
    }
}

#[tokio::test]
async fn maintenance_operations_share_the_thirty_second_budget() {
    for list in [false, true] {
        let mut peer = Peer::start().await;
        let client = client(&peer, true);
        let guard = client.token.state.lock().await;
        tokio::time::pause();
        let task = operation(&client, list);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(25)).await;
        tokio::time::resume();
        drop(guard);
        authorize(&mut peer).await;
        let held = peer.next().await;
        tokio::time::pause();
        let deadline = Instant::now() + Duration::from_secs(5);
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            finish(task).await.unwrap_err().error,
            DocumentError::Unavailable
        );
        assert_deadline(deadline);
        tokio::time::advance(Duration::from_secs(120)).await;
        assert_eq!(peer.count(), 2);
        drop(held);
        tokio::time::resume();
    }
}

#[tokio::test]
async fn expired_maintenance_waiters_never_send_late_requests() {
    let peer = Peer::start().await;
    let client = client(&peer, true);
    let guard = client.token.state.lock().await;
    tokio::time::pause();
    let tasks = [operation(&client, false), operation(&client, true)];
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(tasks.iter().all(|task| !task.is_finished()));
    tokio::time::advance(Duration::from_secs(1)).await;
    for task in tasks {
        assert_eq!(
            finish(task).await.unwrap_err().error,
            DocumentError::Unavailable
        );
    }
    drop(guard);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 0);
}

#[tokio::test]
async fn maintenance_replays_unauthorized_at_most_once_without_transport_retries() {
    for list in [false, true] {
        for status in [401, 403, 429, 500, 503] {
            let mut peer = Peer::start().await;
            let client = client(&peer, true);
            let task = operation(&client, list);
            authorize(&mut peer).await;
            peer.next().await.respond(status, "private diagnostic");
            if status == 401 {
                authorize(&mut peer).await;
                peer.next().await.respond(status, "refused again");
            }
            assert_eq!(
                finish(task).await.unwrap_err().error,
                DocumentError::Unavailable
            );
            assert_eq!(peer.count(), if status == 401 { 4 } else { 2 });
        }
    }
}
