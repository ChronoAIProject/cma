use super::*;

fn history(bound: &Bound, restarts: u32, elapsed: i64, exit: i32) {
    restart(bound, restarts, -10, exit, "Error");
    let mut state = bound.fixture.state.lock().unwrap();
    let status = &mut state.rows.get_mut(POD).unwrap()["status"]["containerStatuses"][0];
    let finished = time::OffsetDateTime::parse(
        status["lastState"]["terminated"]["finishedAt"]
            .as_str()
            .unwrap(),
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    status["lastState"]["terminated"]["startedAt"] = json!((finished
        - time::Duration::seconds(elapsed))
    .format(&time::format_description::well_known::Rfc3339)
    .unwrap());
    status["state"]["running"]["startedAt"] = json!((finished + time::Duration::seconds(1))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap());
}

fn cold(bound: &Bound) -> AgentSandboxBackend {
    AgentSandboxBackend::new(client(&bound.fixture, false), bound.transport.clone())
}

#[tokio::test]
async fn retained_cold_adoption_reconstructs_one_hold_and_due_probe() {
    let bound = fixture(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    history(&bound, 4, 2, 2);
    let backend = cold(&bound);
    backend
        .adopt_with_home(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deliveries(&bound), 2);
    restart(&bound, 5, 0, 2, "Error");
    tokio::time::pause();
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    io(async {
        let results = futures_util::future::join_all(
            (0..8).map(|_| backend.adopt_with_home(&bound.runtime, &bound.request, &home)),
        )
        .await;
        for result in results {
            let failure = hold(result);
            assert_eq!(failure.probe().unwrap().next_probe_at, deadline);
            assert_eq!(failure.consecutive, 1);
        }
    })
    .await;
    assert_eq!(deliveries(&bound), 2);
    tokio::time::advance(Duration::from_secs(15 * 60 - 1)).await;
    let repeated = io(backend.adopt_with_home(&bound.runtime, &bound.request, &home)).await;
    assert_eq!(hold(repeated).probe().unwrap().next_probe_at, deadline);
    tokio::time::advance(Duration::from_secs(1)).await;
    io(async {
        let results = futures_util::future::join_all(
            (0..8).map(|_| backend.adopt_with_home(&bound.runtime, &bound.request, &home)),
        )
        .await;
        for result in results {
            match result {
                Ok(Some(_)) | Err(AdapterError::RuntimeStartupHold(_)) => {}
                other => panic!("unexpected due probe: {other:?}"),
            }
        }
    })
    .await;
    assert_eq!(deliveries(&bound), 3);
}

#[tokio::test]
async fn retained_cold_adoption_ignores_incomplete_or_nonqualifying_history() {
    for case in [
        "below_threshold",
        "zero_exit",
        "long",
        "zero_duration",
        "missing_start",
        "missing_current_start",
        "same_process",
        "replacement",
        "restart_changed",
        "container_changed",
        "owner_changed",
    ] {
        let bound = fixture(true).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        let restarts = if case == "below_threshold" { 2 } else { 4 };
        let elapsed = match case {
            "long" => 60,
            "zero_duration" => 0,
            _ => 2,
        };
        let exit = if case == "zero_exit" { 0 } else { 2 };
        history(&bound, restarts, elapsed, exit);
        {
            let mut state = bound.fixture.state.lock().unwrap();
            let mut pod = state.rows[POD].clone();
            let container = &mut pod["status"]["containerStatuses"][0];
            match case {
                "missing_start" => {
                    container["lastState"]["terminated"]["startedAt"] = json!(null);
                }
                "missing_current_start" => {
                    container["state"]["running"]["startedAt"] = json!(null);
                }
                "same_process" => {
                    container["lastState"]["terminated"]["containerID"] =
                        container["containerID"].clone();
                }
                "restart_changed" => {
                    container["restartCount"] = json!(5);
                }
                "container_changed" => {
                    container["containerID"] = json!("containerd://replacement");
                }
                "replacement" => {
                    pod["metadata"]["uid"] = json!("replacement-pod");
                }
                "owner_changed" => {
                    pod["metadata"]["ownerReferences"][0]["uid"] = json!("foreign-owner");
                }
                _ => {}
            }
            if matches!(
                case,
                "replacement" | "restart_changed" | "container_changed" | "owner_changed"
            ) {
                // The optional history read sees conflicting identity. Ordinary
                // assignment retains its own separate current authority proof.
                state.read_replies.push_back((POD.into(), 200, pod));
            } else {
                state.rows.insert(POD.into(), pod);
            }
        }
        let backend = cold(&bound);
        backend
            .adopt_with_home(&bound.runtime, &bound.request, &home)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(deliveries(&bound), 2, "{case}");
        restart(&bound, restarts + 1, 0, 2, "Error");
        backend
            .adopt_with_home(&bound.runtime, &bound.request, &home)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            deliveries(&bound),
            3,
            "{case} cannot seed immediate recovery"
        );
    }
}

#[tokio::test]
async fn known_serving_assignment_and_fresh_provision_keep_ordinary_delivery() {
    let bound = fixture(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    history(&bound, 4, 2, 2);
    let backend = cold(&bound);
    backend.provision(bound.request.clone()).await.unwrap();
    assert_eq!(deliveries(&bound), 2);
    backend
        .adopt_with_home(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        deliveries(&bound),
        2,
        "current acknowledged assignment wins over historical failures"
    );
}

#[tokio::test]
async fn cold_adoption_with_old_failure_history_proves_healthy_current_serving() {
    let bound = fixture(true).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    history(&bound, 4, 2, 2);
    let backend = AgentSandboxBackend::new(
        client(&bound.fixture, false),
        Arc::new(Serving(bound.transport.clone())),
    );
    backend
        .adopt_with_home(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deliveries(&bound), 2);
    let inspected = backend
        .inspect_native_runtime(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inspected.observation.handle, bound.runtime);
    restart(&bound, 5, 0, 2, "Error");
    backend
        .adopt_with_home(&bound.runtime, &bound.request, &home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        deliveries(&bound),
        3,
        "serving proof clears historical recovery evidence"
    );
}
