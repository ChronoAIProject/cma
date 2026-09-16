use super::super::tests::{claim, request, row, serve, status, template};
use super::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Observer(AtomicUsize);
#[async_trait::async_trait]
impl HomeBackingObserver for Observer {
    async fn capture_original(
        &self,
        reference: &HomeBackingReference,
    ) -> Result<HomeBackingIdentity> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(HomeBackingIdentity {
            reference: reference.clone(),
            backing_generation: "987654321".into(),
        })
    }
    async fn observe_original_absence(
        &self,
        original: &HomeBackingIdentity,
    ) -> Result<crate::HomeBackingAbsence> {
        Ok(crate::HomeBackingAbsence {
            original: original.clone(),
            request_id: "actual-provider-observation".into(),
            response_sha256: "b".repeat(64),
        })
    }
}

fn original(store: &PersistentHomeVolumes) -> HomeBackingIdentity {
    HomeBackingIdentity {
        reference: HomeBackingReference {
            provider_scope: store.allocation_scope().unwrap(),
            home: attachment(),
            pv_name: "original-volume".into(),
            pv_uid: "original-pv-uid".into(),
            csi_driver: "pd.csi.storage.gke.io".into(),
            volume_handle: "projects/original/zones/asia-northeast1-a/disks/original-disk".into(),
        },
        backing_generation: "987654321".into(),
    }
}

#[tokio::test]
async fn acknowledged_home_delete_and_disk_absence_are_separate_verifiable_steps() {
    let fixture = serve(vec![
        row(bound_claim()),
        (200, volume()),
        row(bound_claim()),
        (200, volume()),
        row(bound_claim()),
        status(200),
        status(404),
    ])
    .await;
    let observer = Observer(AtomicUsize::new(0));
    let pin = original(&fixture.store);
    let acknowledgement = fixture
        .store
        .delete_original_home(&pin, "delete-original-home", &observer)
        .await
        .unwrap();
    assert_eq!(acknowledgement.original(), &pin);
    let finality = fixture
        .store
        .observe_home_finality(&acknowledgement, &observer)
        .await
        .unwrap();
    assert_eq!(finality.delete_receipt(), acknowledgement);
    let requests = fixture.finish().await;
    assert!(requests[5].starts_with("DELETE "));
    let body: Value = serde_json::from_str(requests[5].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        body["preconditions"],
        json!({"uid":"original-claim-uid","resourceVersion":"12"})
    );
}

struct UnavailableBacking;
#[async_trait::async_trait]
impl HomeBackingObserver for UnavailableBacking {
    async fn capture_original(&self, _: &HomeBackingReference) -> Result<HomeBackingIdentity> {
        Err(AdapterError::Unavailable(
            "original backing unavailable".into(),
        ))
    }
    async fn observe_original_absence(
        &self,
        _: &HomeBackingIdentity,
    ) -> Result<crate::HomeBackingAbsence> {
        Err(AdapterError::Unavailable(
            "backing still exists or read unavailable".into(),
        ))
    }
}

#[tokio::test]
async fn pvc_absence_without_disk_proof_and_missing_delete_response_remain_held() {
    let fixture = serve(vec![status(404)]).await;
    let acknowledgement = crate::HomeDeleteReceipt {
        original: original(&fixture.store),
        operation_id: "delete-original-home".into(),
        resource_version: "12".into(),
        response_sha256: "a".repeat(64),
    };
    assert!(fixture
        .store
        .observe_home_finality(&acknowledgement, &UnavailableBacking)
        .await
        .is_err());
    fixture.finish().await;
    let fixture = serve(vec![
        row(bound_claim()),
        (200, volume()),
        row(bound_claim()),
        (200, volume()),
        row(bound_claim()),
        status(404),
    ])
    .await;
    assert!(fixture
        .store
        .delete_original_home(
            &original(&fixture.store),
            "delete-original-home",
            &Observer(AtomicUsize::new(0))
        )
        .await
        .is_err());
    assert_eq!(fixture.finish().await.len(), 6);
}

fn bound_claim() -> PersistentVolumeClaim {
    let mut claim = claim("12", "original-claim-uid");
    claim.spec.as_mut().unwrap().volume_name = Some("original-volume".into());
    claim.status = Some(serde_json::from_value(json!({"phase":"Bound"})).unwrap());
    claim
}
fn attachment() -> HomeVolumeAttachment {
    HomeVolumeAttachment {
        request: request(),
        receipt: identity::validate_claim("cma-test", "deployment", &request(), &bound_claim())
            .unwrap(),
    }
}
fn volume() -> Value {
    json!({"apiVersion":"v1","kind":"PersistentVolume",
        "metadata":{"name":"original-volume","uid":"original-pv-uid","resourceVersion":"15"},
        "spec":{"storageClassName":"standard-rwo","persistentVolumeReclaimPolicy":"Delete",
            "claimRef":{"apiVersion":"v1","kind":"PersistentVolumeClaim","namespace":"cma-test","name":attachment().receipt.storage_reference,"uid":"original-claim-uid"},
            "csi":{"driver":"pd.csi.storage.gke.io","volumeHandle":"projects/original/zones/asia-northeast1-a/disks/original-disk"}},
        "status":{"phase":"Bound"}})
}

#[tokio::test]
async fn original_backing_pins_exact_claim_pv_csi_and_numeric_disk_generation() {
    let fixture = serve(vec![
        row(bound_claim()),
        (200, volume()),
        row(bound_claim()),
        (200, volume()),
    ])
    .await;
    let observer = Observer(AtomicUsize::new(0));
    let pin = fixture
        .store
        .original_backing(&attachment(), &observer)
        .await
        .unwrap();
    assert_eq!(pin.reference.home, attachment());
    assert_eq!(pin.reference.pv_uid, "original-pv-uid");
    assert_eq!(pin.backing_generation, "987654321");
    assert_eq!(observer.0.load(Ordering::SeqCst), 1);
    let requests = fixture.finish().await;
    assert!(requests.iter().all(|request| request.starts_with("GET ")));
    assert!(requests[1].starts_with("GET /api/v1/persistentvolumes/original-volume "));
}

#[tokio::test]
async fn original_backing_rejects_replaced_claim_foreign_pv_and_changed_pin() {
    let observer = Observer(AtomicUsize::new(0));
    let mut replacement = bound_claim();
    replacement.metadata.uid = Some("replacement-claim".into());
    let fixture = serve(vec![row(replacement)]).await;
    assert!(fixture
        .store
        .original_backing(&attachment(), &observer)
        .await
        .is_err());
    fixture.finish().await;
    assert_eq!(observer.0.load(Ordering::SeqCst), 0);

    let mut foreign = volume();
    foreign["spec"]["claimRef"]["uid"] = json!("foreign-claim");
    let fixture = serve(vec![row(bound_claim()), (200, foreign)]).await;
    assert!(fixture
        .store
        .original_backing(&attachment(), &observer)
        .await
        .is_err());
    fixture.finish().await;
    assert_eq!(observer.0.load(Ordering::SeqCst), 0);

    let mut replaced_volume = volume();
    replaced_volume["metadata"]["uid"] = json!("replacement-pv");
    let fixture = serve(vec![
        row(bound_claim()),
        (200, volume()),
        row(bound_claim()),
        (200, replaced_volume),
    ])
    .await;
    assert!(fixture
        .store
        .original_backing(&attachment(), &observer)
        .await
        .is_err());
    fixture.finish().await;
    assert_eq!(observer.0.load(Ordering::SeqCst), 1);
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
    assert_eq!(proof.home.receipt.storage_generation, "original-claim-uid");
    assert_eq!(proof.creation_resource_version, "11");
    let requests = fixture.finish().await;
    assert!(requests[2].starts_with("POST "));

    for replies in [
        vec![template(), row(claim("11", "existing-uid"))],
        vec![template(), status(404), status(409)],
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
