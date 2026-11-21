use super::*;

#[tokio::test]
async fn admission_proves_each_native_resource_once_and_replays_without_writes() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let runtime = super::super::runtime_tests::client(&fixture, false);
    let request = super::super::runtime_tests::request(&home, 1);
    let before = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    fixture.state.lock().unwrap().calls.clear();
    let handle = runtime
        .provision_pool_runtime_with_home(&request, &home)
        .await
        .unwrap();
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(
            state.calls.len(),
            4,
            "three fresh authority reads and one fenced publication"
        );
        assert_eq!(
            state
                .calls
                .iter()
                .filter(|(call, _)| call.starts_with("GET "))
                .count(),
            3
        );
        assert_eq!(state.calls.last().unwrap().0, format!("PATCH {MEMBER}"));
        assert_eq!(state.rows[MEMBER]["spec"], before);
    }
    fixture.state.lock().unwrap().calls.clear();
    assert_eq!(
        runtime
            .provision_pool_runtime_with_home(&request, &home)
            .await
            .unwrap(),
        handle
    );
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.calls.len(), 3);
        assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
    }
    fixture.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
        json!("replaced");
    fixture.state.lock().unwrap().calls.clear();
    assert!(runtime
        .provision_pool_runtime_with_home(&request, &home)
        .await
        .is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));
}

#[tokio::test]
async fn admission_rejects_changed_native_authority_before_publishing() {
    for change in [
        "claim_uid",
        "assignment",
        "claim_origin",
        "member_uid",
        "member_origin",
        "member_spec",
        "member_owner",
        "pvc_uid",
        "pvc_owner",
        "pvc_spec",
        "pvc_deleting",
        "request",
        "original",
    ] {
        let fixture = Fixture::new().await;
        let mut claim = assigned_spec(&fixture, &spec()).await;
        let home = adopted(&fixture, &claim).await;
        let runtime = super::super::runtime_tests::client(&fixture, false);
        let mut request = super::super::runtime_tests::request(&home, 1);
        match change {
            "claim_uid" => fixture.edit("sandboxclaims", &claim.name, |v| {
                v["metadata"]["uid"] = json!("other")
            }),
            "assignment" => fixture.edit("sandboxclaims", &claim.name, |v| {
                v["status"]["sandbox"]["name"] = json!("other")
            }),
            "claim_origin" => fixture.edit("sandboxclaims", &claim.name, |v| {
                v["metadata"]["annotations"][NATIVE_HOME_ANNOTATION] = json!("{}")
            }),
            "request" => request.image.uri = "other".into(),
            "original" => claim.uid = "other".into(),
            _ => {
                let mut state = fixture.state.lock().unwrap();
                let (path, pointer, value) = match change {
                    "member_uid" => (MEMBER, "/metadata/uid", json!("other")),
                    "member_origin" => (
                        MEMBER,
                        "/metadata/annotations/cma~1native-home-origin",
                        json!("{}"),
                    ),
                    "member_spec" => (
                        MEMBER,
                        "/spec/podTemplate/spec/containers/0/image",
                        json!("other"),
                    ),
                    "member_owner" => (MEMBER, "/metadata/ownerReferences/0/uid", json!("other")),
                    "pvc_uid" => (VOLUME, "/metadata/uid", json!("other")),
                    "pvc_owner" => (VOLUME, "/metadata/ownerReferences/0/uid", json!("other")),
                    "pvc_spec" => (VOLUME, "/spec/storageClassName", json!("other")),
                    "pvc_deleting" => {
                        state.rows.get_mut(VOLUME).unwrap()["metadata"]["deletionTimestamp"] =
                            json!("2026-10-14T00:00:00Z");
                        (
                            VOLUME,
                            "/metadata/deletionTimestamp",
                            json!("2026-10-14T00:00:00Z"),
                        )
                    }
                    _ => unreachable!(),
                };
                *state
                    .rows
                    .get_mut(path)
                    .unwrap()
                    .pointer_mut(pointer)
                    .unwrap() = value;
            }
        }
        fixture.state.lock().unwrap().calls.clear();
        assert!(
            runtime
                .admit_pool_runtime(&claim, &home, &request)
                .await
                .is_err(),
            "{change}"
        );
        if change != "original" {
            assert!(
                runtime
                    .provision_pool_runtime_with_home(&request, &home)
                    .await
                    .is_err(),
                "persisted home provisioning accepted {change}"
            );
        }
        assert!(
            fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(call, _)| call.starts_with("GET ")),
            "{change}"
        );
    }
}

#[tokio::test]
async fn admission_revalidates_after_member_version_conflict() {
    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let runtime = super::super::runtime_tests::client(&fixture, false);
    let request = super::super::runtime_tests::request(&home, 1);
    fixture
        .state
        .lock()
        .unwrap()
        .conflict
        .push(format!("PATCH {MEMBER}"));
    assert!(matches!(
        runtime
            .provision_pool_runtime_with_home(&request, &home)
            .await,
        Err(crate::AdapterError::Unavailable(_))
    ));
    assert!(
        fixture.state.lock().unwrap().rows[MEMBER]["metadata"]["annotations"]
            .get(super::super::RUNTIME_REQUEST_ANNOTATION)
            .is_none()
    );
    fixture.state.lock().unwrap().rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] =
        json!("replaced-during-conflict");
    fixture.state.lock().unwrap().calls.clear();
    assert!(runtime
        .provision_pool_runtime_with_home(&request, &home)
        .await
        .is_err());
    assert!(fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .all(|(call, _)| call.starts_with("GET ")));

    let fixture = Fixture::new().await;
    let claim = assigned_spec(&fixture, &spec()).await;
    let home = adopted(&fixture, &claim).await;
    let runtime = super::super::runtime_tests::client(&fixture, false);
    let request = super::super::runtime_tests::request(&home, 1);
    fixture
        .state
        .lock()
        .unwrap()
        .conflict
        .push(format!("PATCH {MEMBER}"));
    assert!(runtime
        .provision_pool_runtime_with_home(&request, &home)
        .await
        .is_err());
    fixture.state.lock().unwrap().calls.clear();
    runtime
        .provision_pool_runtime_with_home(&request, &home)
        .await
        .unwrap();
    assert_eq!(fixture.state.lock().unwrap().calls.len(), 4);
}
