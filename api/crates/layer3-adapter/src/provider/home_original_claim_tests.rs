use super::*;
use crate::provider::home_volumes::tests::{claim, request, row, serve, status, template};
use serde_json::{json, Value};

fn original(store: &PersistentHomeVolumes) -> HomeProvisionReceipt {
    let claim = claim("11", "original-claim-uid");
    HomeProvisionReceipt {
        provider_scope: store.allocation_scope().unwrap(),
        home: HomeVolumeAttachment {
            request: request(),
            receipt: identity::validate_claim("cma-test", "deployment", &request(), &claim)
                .unwrap(),
        },
        operation_id: "original-create".into(),
        creation_resource_version: "11".into(),
        creation_response_sha256: crate::provider_request_digest(&claim).unwrap(),
    }
}

#[tokio::test]
async fn original_home_creation_requires_actual_post_acknowledgement() {
    let operation = "home-original-dispatch";
    let mut created = claim("11", "original-claim-uid");
    created.metadata.labels.as_mut().unwrap().insert(
        DISPATCH_LABEL.into(),
        crate::provider::dispatch_marker(operation).unwrap(),
    );
    let fixture = serve(vec![template(), status(404), row(created)]).await;
    let proof = fixture
        .store
        .prepare_receipted(&request(), operation)
        .await
        .unwrap();
    assert_eq!(
        proof.home().receipt.storage_generation,
        "original-claim-uid"
    );
    assert_eq!(
        proof.provider_scope(),
        &fixture.store.allocation_scope().unwrap()
    );
    assert_eq!(proof.creation_resource_version, "11");
    let requests = fixture.finish().await;
    assert!(requests[2].starts_with("POST /api/v1/namespaces/cma-test/persistentvolumeclaims"));
    for replies in [
        vec![template(), row(claim("11", "existing-uid"))],
        vec![template(), status(404), status(409)],
        vec![template(), status(404), status(500)],
    ] {
        let count = replies.len();
        let fixture = serve(replies).await;
        assert!(fixture
            .store
            .prepare_receipted(&request(), operation)
            .await
            .is_err());
        assert_eq!(fixture.finish().await.len(), count);
    }
}

#[tokio::test]
async fn original_home_delete_ack_and_claim_absence_use_only_namespaced_pvc_authority() {
    let fixture = serve(vec![
        row(claim("12", "original-claim-uid")),
        status(200),
        row(claim("13", "original-claim-uid")),
        status(404),
    ])
    .await;
    let pin = original(&fixture.store);
    let ack = fixture
        .store
        .delete_original_home(&pin, "delete-original-home")
        .await
        .unwrap();
    assert_eq!(ack.original(), &pin);
    assert!(fixture.store.observe_home_finality(&ack).await.is_err());
    let finality = fixture.store.observe_home_finality(&ack).await.unwrap();
    finality.validate().unwrap();
    assert_eq!(finality.delete_receipt(), ack);
    assert_eq!(
        finality.cleanup_scope,
        crate::HomeCleanupScope::NamespacedClaim
    );
    let serialized = serde_json::to_value(&finality).unwrap();
    assert_eq!(serialized["cleanup_scope"], "namespaced_claim");
    assert!(!serialized
        .as_object()
        .unwrap()
        .contains_key("backing_absence"));
    let requests = fixture.finish().await;
    assert!(requests.iter().all(|request| request
        .split("\r\n")
        .next()
        .unwrap()
        .contains("/api/v1/namespaces/cma-test/persistentvolumeclaims/")));
    assert!(requests[1].starts_with("DELETE "));
    let body: Value = serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        body["preconditions"],
        json!({"uid":"original-claim-uid","resourceVersion":"12"})
    );
}

#[tokio::test]
async fn original_home_foreign_uid_absent_before_ack_and_lost_delete_ack_remain_held() {
    for replies in [
        vec![row(claim("12", "replacement-claim-uid"))],
        vec![status(404)],
        vec![row(claim("12", "original-claim-uid")), status(404)],
        vec![row(claim("12", "original-claim-uid")), status(500)],
    ] {
        let count = replies.len();
        let fixture = serve(replies).await;
        assert!(fixture
            .store
            .delete_original_home(&original(&fixture.store), "delete-original-home")
            .await
            .is_err());
        let requests = fixture.finish().await;
        assert_eq!(requests.len(), count);
        if count == 1 {
            assert!(requests[0].starts_with("GET "));
        }
    }
    for response in [row(claim("14", "replacement-claim-uid")), status(403)] {
        let fixture = serve(vec![response]).await;
        let ack = crate::HomeDeleteReceipt {
            original: original(&fixture.store),
            operation_id: "delete-original-home".into(),
            resource_version: "12".into(),
            response_sha256: "a".repeat(64),
        };
        assert!(fixture.store.observe_home_finality(&ack).await.is_err());
        fixture.finish().await;
    }
}
