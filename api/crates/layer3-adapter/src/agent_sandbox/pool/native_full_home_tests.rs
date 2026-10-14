use super::*;

fn full_home_spec() -> NativePoolSpec {
    let mut spec = spec();
    spec.home.mode = HomeVolumeMode::NativeVctFullHomeV2;
    spec.image.labels.insert(
        crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
        "2".into(),
    );
    spec
}

#[test]
fn full_home_policy_has_distinct_wire_and_digest_and_retains_v1_identity() {
    let mut request = crate::HomeVolumeRequest {
        volume_id: "home-one".into(),
        sandbox_id: "sbx_one".into(),
        policy: spec().home,
        scope: None,
    };
    assert_eq!(
        serde_json::to_value(&request.policy).unwrap(),
        json!({
            "mode":"native_vct_empty_v1", "storage_class":"cma-home", "capacity_gib":4,
        })
    );
    let before = request.computed_sha256();
    assert_eq!(
        before,
        "eb9c9ca545f8f62d68ffacb6fda4bf34612e95ab6b86e027aa3cdf62896c1a77"
    );
    request.policy = full_home_spec().home;
    assert_eq!(
        serde_json::to_value(&request.policy).unwrap(),
        json!({
            "mode":"native_vct_full_home_v2", "storage_class":"cma-home", "capacity_gib":4,
        })
    );
    assert_ne!(before, request.computed_sha256());
    let decoded: crate::HomeVolumeRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert_eq!(decoded, request);
}

#[tokio::test]
async fn full_home_template_requires_capable_image_and_uses_distinct_pool_identity() {
    let fixture = Fixture::new().await;
    let mut full = full_home_spec();
    let mut old = full.clone();
    old.home.mode = HomeVolumeMode::NativeVctEmptyV1;
    assert_ne!(
        model::digest(&fixture.client.config, &old).unwrap(),
        model::digest(&fixture.client.config, &full).unwrap()
    );
    full.image
        .labels
        .remove(crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL);
    assert!(full.validate().is_err());
    let claim = assigned_spec(&fixture, &full_home_spec()).await;
    let template = fixture.row("sandboxtemplates", &claim.request.pool.template_name);
    let pod = &template["spec"]["podTemplate"]["spec"];
    assert_eq!(
        pod["initContainers"][0]["command"],
        json!([
            "/usr/local/bin/layer4-runtime",
            "--initialize-home-tree",
            "/mnt/cma-home"
        ])
    );
    assert_eq!(
        pod["containers"][0]["volumeMounts"],
        json!([
            {"name":"home","mountPath":"/home","subPath":"home","readOnly":false},
            {"name":"home","mountPath":"/var/lib/cma-runtime","subPath":"state","readOnly":false},
            {"name":"home","mountPath":"/var/lib/cma-transfer","subPath":"transfer","readOnly":false}
        ])
    );
    assert_eq!(
        template["spec"]["volumeClaimTemplates"][0]["metadata"]["annotations"]["cma/home-layout"],
        "native-vct-full-home-v2"
    );
    let home = adopted(&fixture, &claim).await;
    homes(&fixture).verify(&home).await.unwrap();
    assert_eq!(homes(&fixture).inventory().await.unwrap(), vec![home]);
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn full_home_pause_resume_keeps_exact_claim_sandbox_pvc_and_mounts() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &full_home_spec()).await;
    let home = adopted(&fixture, &claim).await;
    let original = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    let volume = fixture.state.lock().unwrap().rows[VOLUME].clone();
    let runtime = super::super::runtime_tests::client(&fixture, false);
    let request = super::super::runtime_tests::request(&home, 1);
    assert_eq!(request.volumes[0].mount_path, "/home");
    assert_eq!(request.env["HOME"], "/home/cma");
    assert_eq!(request.env["CHRONO_RUNTIME_HOME"], "/home/cma");
    let handle = runtime
        .admit_pool_runtime(&claim, &home, &request)
        .await
        .unwrap();
    assert_eq!(
        runtime.provision_pool_runtime(&request).await.unwrap(),
        Some(handle.clone())
    );
    runtime.suspend(&handle, &request).await.unwrap();
    runtime.verify_suspended(&handle, &request).await.unwrap();
    // Current deployment defaults may change; the retained pool determines layout.
    let restarted = super::super::runtime_tests::client(&fixture, true);
    let next = super::super::runtime_tests::request(&home, 2);
    restarted.resume(&handle, &request, &next).await.unwrap();
    restarted
        .validate_request(&handle, &next)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture.client.recover_home(&claim).await.unwrap(),
        Some(home.clone())
    );
    assert_eq!(fixture.state.lock().unwrap().rows[MEMBER]["spec"], original);
    assert_eq!(fixture.state.lock().unwrap().rows[VOLUME], volume);
    assert_eq!(
        fixture.row("sandboxclaims", &claim.name)["metadata"]["uid"],
        claim.uid
    );
    homes(&fixture).verify(&home).await.unwrap();
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn full_home_refuses_v1_annotation_or_mount_even_with_same_storage_identity() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &full_home_spec()).await;
    let home = adopted(&fixture, &claim).await;
    let mut request = super::super::runtime_tests::request(&home, 1);
    request.volumes[0].mount_path = "/home/cma".into();
    request.metadata.remove("cma/spec-sha256");
    request.metadata.insert(
        "cma/spec-sha256".into(),
        crate::hosted::provider_request_fingerprint(&request).unwrap(),
    );
    assert!(super::super::runtime_tests::client(&fixture, false)
        .admit_pool_runtime(&claim, &home, &request)
        .await
        .is_err());
    fixture.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["annotations"]
        ["cma/home-layout"] = json!("native-vct-empty-v1");
    assert!(homes(&fixture).verify(&home).await.is_err());
    no_storage_effects(&fixture);
}

#[tokio::test]
async fn changing_native_default_preserves_retained_v1_request_and_runtime_mount() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let routes = crate::RoutedHomeVolumes::new(
        None,
        Some((std::sync::Arc::new(homes(&fixture)), full_home_spec().home)),
    )
    .unwrap();
    assert_eq!(
        routes.reserve_for(true, &home.request).unwrap(),
        home.request
    );
    assert_eq!(
        super::super::runtime_tests::request(&home, 2).volumes[0].mount_path,
        "/home/cma"
    );
    let mut new = home.request.clone();
    new.scope = None;
    let new = routes.reserve_for(true, &new).unwrap();
    assert!(new.policy.mounts_full_home());
    assert_ne!(home.request.computed_sha256(), new.computed_sha256());
    assert_eq!(new.scope, home.request.scope);
    routes.verify(&home).await.unwrap();
    no_storage_effects(&fixture);
}
