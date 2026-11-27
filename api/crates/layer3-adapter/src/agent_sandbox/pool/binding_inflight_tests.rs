use super::*;

#[tokio::test]
async fn bound_status_accepts_an_identical_assignment_replay_during_exchange() {
    let fixture = fixture(true, true).await;
    fixture
        .adapter
        .adopt_runtime(&fixture.adoption)
        .await
        .unwrap()
        .unwrap();
    let held = fixture.stream.gate.acquire().await.unwrap();
    let status = tokio::spawn({
        let adapter = fixture.adapter.clone();
        let handle = fixture.bound.runtime.clone();
        async move { adapter.runtime_status(&handle).await }
    });
    received(&fixture).await;

    let replay = fixture
        .backend
        .provision(fixture.bound.request.clone())
        .await
        .unwrap();
    assert_eq!(replay, fixture.bound.runtime);
    {
        let state = fixture.bound.fixture.state.lock().unwrap();
        assert_eq!(state.assignments.len(), 3);
        assert_eq!(state.assignments[1], state.assignments[2]);
    }
    drop(held);

    let status = status.await.unwrap().unwrap();
    assert_eq!(status.boot_id, "a".repeat(32));
    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn combined_native_inspection_revalidates_the_current_status_reply() {
    for boundary in [false, true] {
        for change in [
            "none",
            "claim_uid",
            "assignment",
            "pod",
            "container",
            "pvc",
            "image",
            "generation",
        ] {
            let fixture = fixture(false, true).await;
            let (path, pointer, value) = mutation(&fixture, change);
            let state = fixture.bound.fixture.state.clone();
            let fired = Arc::new(AtomicBool::new(false));
            let observed = fired.clone();
            let result = ProviderHostedControlTransport::with_status_hook(
                move |point| {
                    if point == boundary
                        && !observed.swap(true, Ordering::SeqCst)
                        && change != "none"
                    {
                        *state
                            .lock()
                            .unwrap()
                            .rows
                            .get_mut(&path)
                            .unwrap()
                            .pointer_mut(pointer)
                            .unwrap() = value.clone();
                    }
                },
                fixture.adapter.inspect_runtime(&fixture.adoption),
            )
            .await;
            assert!(fired.load(Ordering::SeqCst));
            assert_eq!(
                result.is_ok(),
                change == "none",
                "{change}, after={boundary}: {result:?}"
            );
            assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
            assert!(fixture
                .bound
                .fixture
                .state
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|(method, _)| method.starts_with("GET ")));
        }
    }
}

fn mutation(fixture: &ReusedInspection, change: &str) -> (String, &'static str, serde_json::Value) {
    match change {
        "none" => (POD.into(), "/metadata/uid", json!(null)),
        "claim_uid" => (
            format!(
                "{}/sandboxclaims/{}",
                crate::agent_sandbox::pool::fixture::prefix(),
                fixture.bound.claim.name
            ),
            "/metadata/uid",
            json!("replacement"),
        ),
        "assignment" => (
            format!(
                "{}/sandboxclaims/{}",
                crate::agent_sandbox::pool::fixture::prefix(),
                fixture.bound.claim.name
            ),
            "/status/sandbox/name",
            json!("other-member"),
        ),
        "pod" => (POD.into(), "/metadata/uid", json!("replacement-pod")),
        "container" => (
            POD.into(),
            "/status/containerStatuses/0/containerID",
            json!("containerd://replacement"),
        ),
        "pvc" => (VOLUME.into(), "/metadata/uid", json!("replacement-pvc")),
        "image" => (
            MEMBER.into(),
            "/spec/podTemplate/spec/containers/0/image",
            json!("replacement-image"),
        ),
        "generation" => (
            MEMBER.into(),
            "/metadata/annotations/cma~1runtime-generation",
            json!("42"),
        ),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn current_adoption_status_attempt_rejects_inflight_native_authority_changes() {
    let mut accepted = Vec::new();
    for fresh in [false, true] {
        for boundary in [false, true] {
            for change in [
                "none",
                "claim_uid",
                "assignment",
                "pod",
                "container",
                "pvc",
                "image",
                "generation",
            ] {
                let mut fixture = fixture(false, true).await;
                if fresh {
                    fixture
                        .bound
                        .fixture
                        .state
                        .lock()
                        .unwrap()
                        .rows
                        .get_mut(POD)
                        .unwrap()["metadata"]["uid"] = json!("fresh-process");
                    fixture
                        .transport
                        .retire(&fixture.bound.runtime)
                        .await
                        .unwrap();
                }
                fixture.adoption = fixture
                    .adoption
                    .clone()
                    .with_expected_image(fixture.adoption.spec.runtime_image.clone().unwrap());
                let assignments = fixture
                    .bound
                    .fixture
                    .state
                    .lock()
                    .unwrap()
                    .assignments
                    .len();
                let (path, pointer, value) = mutation(&fixture, change);
                let state = fixture.bound.fixture.state.clone();
                let fired = Arc::new(AtomicBool::new(false));
                let observed = fired.clone();
                let hook = move |point| {
                    if point == boundary
                        && !observed.swap(true, Ordering::SeqCst)
                        && change != "none"
                    {
                        *state
                            .lock()
                            .unwrap()
                            .rows
                            .get_mut(&path)
                            .unwrap()
                            .pointer_mut(pointer)
                            .unwrap() = value.clone();
                    }
                };
                // This is the production reconciler's ordered composite, inside
                // the same reserved deadline scope; no earlier status is used.
                let result = ProviderHostedControlTransport::with_status_hook(
                    hook,
                    crate::binding_diagnostics::scope(
                        tokio::time::Instant::now() + std::time::Duration::from_secs(20),
                        async {
                            fixture
                                .adapter
                                .adopt_runtime(&fixture.adoption)
                                .await?
                                .ok_or_else(|| AdapterError::Rejected("absent".into()))?;
                            if fresh {
                                fixture.task.abort();
                                let socket = fixture
                                    .transport
                                    .seed_native_status_connection(&fixture.bound.runtime)
                                    .await;
                                fixture.task = tokio::spawn(serve(
                                    socket,
                                    fixture.stream.clone(),
                                    fixture.bound.fixture.state.clone(),
                                ));
                            }
                            tokio::time::timeout(
                                std::time::Duration::from_secs(2),
                                fixture.adapter.runtime_status(&fixture.bound.runtime),
                            )
                            .await
                            .expect("fixture timeout must not count as authority refusal")
                        },
                    ),
                )
                .await;
                assert!(
                    fired.load(Ordering::SeqCst),
                    "hook must run inside current status: {change}, after={boundary}, fresh={fresh}, result={result:?}"
                );
                if boundary {
                    assert_eq!(fixture.stream.requests.load(Ordering::SeqCst), 1);
                }
                let state = fixture.bound.fixture.state.lock().unwrap();
                assert_eq!(state.assignments.len(), assignments + usize::from(fresh));
                assert!(state
                    .calls
                    .iter()
                    .all(|(method, _)| !method.starts_with("DELETE ")));
                if change == "none" {
                    assert!(
                        result.is_ok(),
                        "unchanged control after={boundary}, fresh={fresh}: {result:?}"
                    );
                } else if result.is_ok() {
                    accepted.push(format!("{change}/{boundary:?}/fresh={fresh}"));
                }
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "stale authority was accepted: {accepted:?}"
    );
}
