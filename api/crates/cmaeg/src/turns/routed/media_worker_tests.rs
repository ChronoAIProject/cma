use super::*;
use crate::turns::routed::{
    adapter_test_server::HttpServer,
    custom_http_media_tests::inline_media,
    delivery_tests::accepted,
    test_support::{fixture, supported, Setup},
};
use cmaeg_core::media_continuation::MediaArtifactClaim;
use serde_json::json;
#[path = "media_worker_restart_tests.rs"]
mod restart;
struct NoDelivery;
impl MediaResponseDelivery for NoDelivery {
    fn deliver<'a>(
        &'a self,
        _: &'a AdmittedPlan,
        _: &'a StoredTarget,
        _: &'a mut MediaLease,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async { panic!("upload recovery cannot deliver responses") })
    }
    fn finish<'a>(
        &'a self,
        _: &'a AdmittedPlan,
        _: &'a StoredTarget,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async { panic!("upload recovery cannot close streams") })
    }
}
#[tokio::test]
async fn media_upload_recovers_accepted_receipt_without_refetching_unavailable_original() {
    let mut activity = fixture::custom(None);
    let cmaeg_core::activity::ActivityKind::Event { attachments, .. } = &mut activity.kind else {
        unreachable!()
    };
    // This source deliberately does not exist in the Custom payload. A refetch
    // would fail; only a correctly recovered durable receipt can complete.
    attachments.push(serde_json::from_value(json!({"reference":"file","kind":"file","filename":"original.txt","source":{"type":"inline","pointer":"/missing"}})).unwrap());
    let setup = Setup::with_request(
        supported(),
        fixture::request(
            |v| {
                v["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_existing"})
            },
            activity,
        ),
    )
    .await;
    let original_lease = accepted(&setup).await;
    let store = MediaObligationStore(setup.journal().clone());
    store
        .initialize(&setup.plan, setup.target(), 0, None, None)
        .await
        .unwrap();
    let mut lease = store
        .claim(&setup.plan, setup.target(), 0, "media-recovery")
        .await
        .unwrap();
    let original = MediaArtifactClaim {
        attachment_index: 0,
        kind: MediaContinuationKind::Original,
        content_sha256: sha256(b"file"),
        source_sha256: sha256(b"file"),
        name: "original.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: 4,
        text_sha256: None,
    };
    store.freeze_original(&mut lease, original).await.unwrap();
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.event_context = Some(
        json!({"id":"att_fixture","agent_id":"agt_existing","name":"original.txt","path":"/home/attachments/agt_existing/att_fixture/original.txt","size_bytes":4,"media_type":"text/plain","sha256":sha256(b"file")}),
    );
    let registry = crate::providers::ProviderRegistry::parse(&json!([{
        "slug":"cma_codex", "base_url":http.base, "kind":"responses_http", "max_inflight":2,
        "paths":{"agent_card":"/card", "binding":"/api/v1/cmaeg/bindings/{binding_id}", "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}", "responses":"/api/v2/responses"}
    }]).to_string()).unwrap();
    let providers =
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 2)
            .unwrap()
            .with_cma_origin_fixture(&http.base);
    let worker = MediaWorker {
        index: MediaWorkIndex::new(setup.journal().clone(), setup.server.store()),
        plans: setup.server.store(),
        store: store.clone(),
        authority: setup.current.clone(),
        providers: Arc::new(providers),
        media: inline_media(),
        delivery: Arc::new(NoDelivery),
        worker: "media-recovery".into(),
    };
    worker
        .upload(&setup.plan, setup.target(), &mut lease)
        .await
        .unwrap();
    assert_eq!(lease.state().upload.as_ref().unwrap().id, "att_fixture");
    let requests = http.seen();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert!(requests[0].uri.ends_with("/media/0/attachment"));
    setup.journal().guard(&original_lease).await.unwrap();
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}
