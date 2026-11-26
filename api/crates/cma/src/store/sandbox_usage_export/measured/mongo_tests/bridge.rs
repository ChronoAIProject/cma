use super::*;
use layer2_core::sandbox::{RuntimeBinding, SandboxHome};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn observer_bridge_is_atomic_and_disabled_capture_gap_is_never_certified() {
    let f = Fixture::new().await;
    let mut candidate = f.bind().await;
    candidate.valid_until = None;
    candidate.observed_at = f.minute;
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .delete_many(doc! {})
        .await
        .unwrap();
    f.store
        .collection(INVENTORY)
        .delete_many(keys::page(&keys::bridge(&candidate.id), None).unwrap())
        .await
        .unwrap();
    let reporting = usage_reports::ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        f.db.name(),
        &[23; 32],
    )
    .await
    .unwrap();
    reporting
        .collection(usage_reports::CONTROL)
        .update_one(
            doc! {"_id":"observer"},
            doc! {"$set":{"lease_until":0_i64,"holder":""}},
        )
        .await
        .unwrap();
    assert!(reporting
        .lease("observer", "fixture-holder", 70)
        .await
        .unwrap());
    let mut parent = crate::store::sandbox_mongo_integration_tests::sandbox(&f.stream.sandbox_id);
    parent.owner = layer2_core::SubjectId::new(&f.stream.owner);
    parent.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-fixture".into(),
        provision_generation: Some(1),
        runtime_version: "fixture-image".into(),
        bound_at: layer2_core::now_rfc3339(),
        boot_id: None,
    });
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 1,
            template_name: "fixture".into(),
            template_generation: "fixture".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture".into(),
        storage_generation: candidate.pvc_uid.clone(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    f.db.collection::<Document>("sandboxes").insert_one(doc! {"_id":parent.id.as_str(),"owner":parent.owner.as_str(),"revision":parent.revision as i64}).await.unwrap();
    let before = f.rows(INVENTORY).await;
    reporting
        .reconcile_bindings_with_evidence(
            "fixture-holder",
            parent.id.as_str(),
            vec![candidate.clone()],
            f.minute,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        f.rows(INVENTORY).await,
        before,
        "off makes no evidence writes"
    );
    for offset in [60, 120] {
        reporting
            .reconcile_bindings_with_evidence(
                "fixture-holder",
                parent.id.as_str(),
                vec![candidate.clone()],
                f.minute + offset,
                Some(&parent),
            )
            .await
            .unwrap();
    }
    for offset in [180, 240] {
        reporting
            .reconcile_bindings_with_evidence(
                "fixture-holder",
                parent.id.as_str(),
                vec![candidate.clone()],
                f.minute + offset,
                None,
            )
            .await
            .unwrap();
    }
    for offset in [300, 360] {
        reporting
            .reconcile_bindings_with_evidence(
                "fixture-holder",
                parent.id.as_str(),
                vec![candidate.clone()],
                f.minute + offset,
                Some(&parent),
            )
            .await
            .unwrap();
    }
    let live: Binding = decode(
        &reporting
            .collection(sandbox_metric_bindings::COLLECTION)
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.rows(sandbox_metric_bindings::COLLECTION).await.len(),
        1,
        "reporting interval stays continuous"
    );
    let proofs: Vec<Document> = f
        .store
        .collection(INVENTORY)
        .find(keys::page(&keys::bridge(&live.id), None).unwrap())
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let proofs: Vec<BindingProof> = proofs.iter().map(|r| decode(r).unwrap()).collect();
    assert_eq!(proofs.len(), 2);
    assert_eq!(
        (proofs[0].verified_from, proofs[0].verified_until),
        (f.minute + 60, f.minute + 120)
    );
    assert_eq!(
        (proofs[1].verified_from, proofs[1].verified_until),
        (f.minute + 300, f.minute + 360)
    );
    let before = f.rows(INVENTORY).await;
    f.db.collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$inc":{"revision":1_i64}},
        )
        .await
        .unwrap();
    assert!(reporting
        .reconcile_bindings_with_evidence(
            "fixture-holder",
            parent.id.as_str(),
            vec![candidate],
            f.minute + 420,
            Some(&parent)
        )
        .await
        .is_err());
    assert_eq!(f.rows(INVENTORY).await, before);
    f.db.drop().await.unwrap();
}
