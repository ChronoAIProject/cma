use super::*;
use serde_json::Value;

#[path = "creation_pending.rs"]
mod pending;

fn reserved_body() -> Value {
    let mut body = create_body();
    body["record_id"] = uuid::Uuid::new_v4().to_string().into();
    body["credentials"] = json!({"agent_key":uuid::Uuid::new_v4().to_string(),"channel_key":uuid::Uuid::new_v4().to_string()});
    body["sources"][0]["route_ids"] = json!([]);
    body
}

#[tokio::test]
async fn reserved_creation_replays_across_replicas_and_restart_without_rebinding() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let client = Client::new();
    let body = reserved_body();
    let record_id = body["record_id"].as_str().unwrap();
    let (first, stop, task) = app(&provider, &cma, &nyxid).await;
    let response = client
        .post(format!("{first}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    assert_eq!(created["record_id"], record_id);
    assert_eq!(created["definition"]["creation"]["ready"], true);
    let binds = provider.requests().len();
    stop.send(()).unwrap();
    task.await.unwrap();
    let (second, stop, task) = app(&provider, &cma, &nyxid).await;
    for (method, url, input) in [
        (
            reqwest::Method::GET,
            format!("{second}/v1/channel-records/{record_id}"),
            None,
        ),
        (
            reqwest::Method::POST,
            format!("{second}/v1/channels"),
            Some(&body),
        ),
    ] {
        let mut request = client.request(method, url).bearer_auth("token-alice");
        if let Some(body) = input {
            request = request.json(body);
        }
        let response = request.send().await.unwrap();
        assert!(response.status().is_success());
        let recovered: Value = response.json().await.unwrap();
        assert_eq!(recovered["channel_id"], created["channel_id"]);
        assert_eq!(recovered["endpoints"], created["endpoints"]);
        assert_eq!(
            recovered["provider"]["binding_id"],
            created["provider"]["binding_id"]
        );
    }
    assert_eq!(provider.requests().len(), binds);
    assert_eq!(cma.records().len(), 1);
    let url = format!("{second}/v1/channel-records/{record_id}");
    assert_eq!(
        client
            .get(&url)
            .bearer_auth("token-bob")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let mut changed = body.clone();
    changed["name"] = "changed".into();
    assert_eq!(
        client
            .post(format!("{second}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&changed)
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn cached_reserved_creation_does_not_replay_a_channel_deleted_by_another_replica() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (first, stop_first, first_task) = app(&provider, &cma, &nyxid).await;
    let (second, stop_second, second_task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let body = reserved_body();
    let record_id = body["record_id"].as_str().unwrap();
    let create = || {
        client
            .post(format!("{first}/v1/channels"))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", record_id)
            .json(&body)
    };

    let response = create().send().await.unwrap();
    assert_eq!(response.status(), 201);
    let original: Value = response.json().await.unwrap();
    let replay = create().send().await.unwrap();
    assert_eq!(replay.status(), 201);
    assert_eq!(replay.json::<Value>().await.unwrap(), original);
    assert_eq!(provider.requests().len(), 1);

    let deleted = client
        .delete(format!(
            "{second}/v1/channels/{}?expected_version={}",
            original["channel_id"].as_str().unwrap(),
            original["version"].as_u64().unwrap(),
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    assert!(cma.records().is_empty());

    let replay = create().send().await.unwrap();
    assert_eq!(replay.status(), 409);
    assert_eq!(replay.json::<Value>().await.unwrap()["code"], "conflict");
    assert!(cma.records().is_empty());
    let effects = provider.requests();
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0].method, "PUT");
    assert_eq!(effects[1].method, "DELETE");

    stop_first.send(()).unwrap();
    stop_second.send(()).unwrap();
    first_task.await.unwrap();
    second_task.await.unwrap();
}

#[tokio::test]
async fn failed_binding_retains_encrypted_cleanup_identity_and_blocks_events() {
    failed_binding_creation(
        FakeProviderConfig {
            binding_failures: [(0, (422, json!({"code": "profile_refused"})))].into(),
            ..Default::default()
        },
        502,
        "provider_rejected",
    )
    .await;
}

#[tokio::test]
async fn invalid_binding_profile_is_refused_and_retains_only_cleanup_identity() {
    failed_binding_creation(
        FakeProviderConfig {
            binding_failures: [(0, (400, json!({"code": "invalid_binding"})))].into(),
            ..Default::default()
        },
        400,
        "provider_rejected",
    )
    .await;
}

#[tokio::test]
async fn uncertain_binding_is_listed_as_incomplete_and_remains_recoverable() {
    failed_binding_creation(
        FakeProviderConfig {
            binding_failures: [(0, (503, json!({})))].into(),
            ..Default::default()
        },
        503,
        "provider_unavailable",
    )
    .await;
}

async fn failed_binding_creation(config: FakeProviderConfig, status: u16, code: &str) {
    let provider = FakeProvider::start(config).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let resolver = state.resolver.clone();
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    let body = reserved_body();
    let channels_url = format!("{base}/v1/channels");
    let response = client
        .post(&channels_url)
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), status);
    assert_eq!(response.json::<Value>().await.unwrap()["code"], code);
    let recovered: Value = client
        .get(format!(
            "{base}/v1/channel-records/{}",
            body["record_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["definition"]["creation"]["ready"], false);
    let id = cmaeg_core::ids::ChannelId::new(recovered["channel_id"].as_str().unwrap()).unwrap();
    let refused = resolver.resolve(id.as_str()).await.unwrap_err();
    assert_eq!(
        refused,
        cmaeg::channels::resolver::ResolveError::CreationPending
    );
    assert_eq!(refused.problem().status, 409);
    assert!(refused
        .problem()
        .detail
        .unwrap()
        .contains("channel_creation_pending"));

    let sibling = client
        .post(&channels_url)
        .bearer_auth("token-alice")
        .json(&reserved_body())
        .send()
        .await
        .unwrap();
    assert_eq!(sibling.status(), 201);
    let sibling: Value = sibling.json().await.unwrap();
    let list = client
        .get(&channels_url)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let page: Value = list.json().await.unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let incomplete = items
        .iter()
        .find(|item| item["record_id"] == body["record_id"])
        .unwrap();
    assert_eq!(incomplete["definition"]["creation"]["ready"], false);
    assert_eq!(incomplete["channel_id"], recovered["channel_id"]);
    assert_eq!(incomplete["version"], recovered["version"]);
    assert!(incomplete.get("endpoints").is_none());
    let ready = items
        .iter()
        .find(|item| item["record_id"] == sibling["record_id"])
        .unwrap();
    assert_eq!(ready["definition"]["creation"]["ready"], true);
    assert_eq!(ready["endpoints"], sibling["endpoints"]);

    let url = format!(
        "{base}/v1/channels/{id}?expected_version={}",
        recovered["version"]
    );
    assert_eq!(
        client
            .delete(url)
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(cma.records().len(), 1);
    assert_eq!(provider.requests().last().unwrap().method, "DELETE");
    stop.send(()).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn concurrent_reserved_creates_have_one_record_and_binding_identity() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (a, stop_a, task_a) = app(&provider, &cma, &nyxid).await;
    let (b, stop_b, task_b) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let body = reserved_body();
    let (a, b) = tokio::join!(
        client
            .post(format!("{a}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&body)
            .send(),
        client
            .post(format!("{b}/v1/channels"))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.status(), 201);
    assert_eq!(b.status(), 201);
    let a: Value = a.json().await.unwrap();
    let b: Value = b.json().await.unwrap();
    assert_eq!(a["channel_id"], b["channel_id"]);
    assert_eq!(a["provider"]["binding_id"], b["provider"]["binding_id"]);
    assert_eq!(cma.records().len(), 1);
    stop_a.send(()).unwrap();
    stop_b.send(()).unwrap();
    task_a.await.unwrap();
    task_b.await.unwrap();
}

#[tokio::test]
async fn publication_conflict_does_not_report_a_pending_creation_as_completed() {
    for unavailable_readback in [false, true] {
        pending_creation_after_publication_conflict(unavailable_readback).await;
    }
}

async fn pending_creation_after_publication_conflict(unavailable_readback: bool) {
    use std::sync::atomic::Ordering;

    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (base, stop, task) = app(&provider, &cma, &nyxid).await;
    let client = Client::new();
    let body = reserved_body();
    cma.put_conflicts.store(1, Ordering::SeqCst);
    if unavailable_readback {
        // Initial lookup and the publication's own exact-write check succeed;
        // fail only creation's subsequent confirmation of a concurrent winner.
        *cma.get_failure_at.lock().unwrap() = Some(2);
    }
    let refused = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(refused.json::<Value>().await.unwrap()["code"], "conflict");
    assert!(cma.get_failure_at.lock().unwrap().is_none());
    let retained: Value = client
        .get(format!(
            "{base}/v1/channel-records/{}",
            body["record_id"].as_str().unwrap(),
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retained["definition"]["creation"]["ready"], false);
    assert_eq!(cma.records().len(), 1);
    let retry = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), 201);
    let completed: Value = retry.json().await.unwrap();
    assert_eq!(completed["channel_id"], retained["channel_id"]);
    assert_eq!(completed["definition"]["creation"]["ready"], true);
    assert_eq!(cma.records().len(), 1);
    stop.send(()).unwrap();
    task.await.unwrap();
}
