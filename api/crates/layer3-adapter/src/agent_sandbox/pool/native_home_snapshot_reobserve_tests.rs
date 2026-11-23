use super::*;
use crate::agent_sandbox::pool::read_fixture::{failure, Effects, Gate};
use std::{collections::BTreeMap, time::Duration};

const CONTENT: &str = "/apis/snapshot.storage.k8s.io/v1/volumesnapshotcontents/content";
const PVC: &str = "/api/v1/namespaces/cma-test/persistentvolumeclaims/home";

fn install(fixture: &Fixture, previous: &NativeHomeSnapshotObservation) {
    let mut snapshot = desired_snapshot("cma-test", "cma-test", &previous.request).unwrap();
    snapshot.metadata.uid = Some(previous.snapshot_uid.clone());
    snapshot.data["status"] = json!({"readyToUse": previous.ready});
    if let Some(cut) = &previous.creation_time {
        snapshot.data["status"]["creationTime"] = json!(cut);
    }
    let mut state = fixture.state.lock().unwrap();
    if let Some(name) = &previous.snapshot_content_name {
        snapshot.data["status"]["boundVolumeSnapshotContentName"] = json!(name);
        state.rows.insert(
            CONTENT.into(),
            json!({
                "apiVersion":"snapshot.storage.k8s.io/v1", "kind":"VolumeSnapshotContent",
                "metadata":{"name":name,"uid":previous.snapshot_content_uid},
                "spec":{"volumeSnapshotRef":{"name":previous.snapshot_name,
                    "namespace":"cma-test","uid":previous.snapshot_uid}},
                "status":{"snapshotHandle":previous.underlying_copy_handle}
            }),
        );
    }
    state.rows.insert(
        snapshot_path(&previous.request),
        serde_json::to_value(snapshot).unwrap(),
    );
}

fn assert_calls(fixture: &Fixture, expected: &[String]) {
    let state = fixture.state.lock().unwrap();
    let calls: Vec<_> = state.calls.iter().map(|(call, _)| call.clone()).collect();
    assert_eq!(calls, expected);
    assert!(state.calls.iter().all(|(_, body)| body.is_null()));
}

fn ready_reads(previous: &NativeHomeSnapshotObservation) -> Vec<String> {
    vec![
        format!("GET {}", snapshot_path(&previous.request)),
        format!("GET {CONTENT}"),
    ]
}

#[tokio::test]
async fn same_name_replacements_refuse_through_the_adapter_without_effects() {
    for changed in ["snapshot_uid", "content_uid", "handle", "cut", "ready"] {
        let fixture = Fixture::new().await;
        let original = observation(true);
        install(&fixture, &original);
        {
            let mut state = fixture.state.lock().unwrap();
            match changed {
                "snapshot_uid" => {
                    state
                        .rows
                        .get_mut(&snapshot_path(&original.request))
                        .unwrap()["metadata"]["uid"] = json!("replacement-snapshot");
                    state.rows.get_mut(CONTENT).unwrap()["spec"]["volumeSnapshotRef"]["uid"] =
                        json!("replacement-snapshot");
                }
                "content_uid" => {
                    state.rows.get_mut(CONTENT).unwrap()["metadata"]["uid"] =
                        json!("replacement-content");
                }
                "handle" => {
                    state.rows.get_mut(CONTENT).unwrap()["status"]["snapshotHandle"] =
                        json!("replacement-copy");
                }
                "cut" => {
                    state
                        .rows
                        .get_mut(&snapshot_path(&original.request))
                        .unwrap()["status"]["creationTime"] = json!("2026-11-17T00:01:00Z");
                }
                _ => {
                    state
                        .rows
                        .get_mut(&snapshot_path(&original.request))
                        .unwrap()["status"]["readyToUse"] = json!(false);
                }
            }
        }
        assert!(
            fixture
                .client
                .observe_native_home_snapshot(&original)
                .await
                .is_err(),
            "{changed}"
        );
        assert_calls(&fixture, &ready_reads(&original));
    }
}

#[tokio::test]
async fn content_replacement_between_reads_cannot_complete_an_original_observation() {
    let fixture = Fixture::new().await;
    let original = observation(true);
    install(&fixture, &original);
    let gate = Gate::new();
    let effects = Effects::new(&fixture, BTreeMap::from([(CONTENT.into(), gate.clone())]));
    let pools = effects.pools(&fixture);
    let previous = original.clone();
    let read = tokio::spawn(async move { pools.observe_native_home_snapshot(&previous).await });
    gate.wait().await;
    fixture.state.lock().unwrap().rows.get_mut(CONTENT).unwrap()["spec"]["volumeSnapshotRef"]
        ["uid"] = json!("replacement-snapshot");
    gate.respond();
    assert!(tokio::time::timeout(Duration::from_secs(2), read)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert_eq!(*effects.calls.lock().unwrap(), ready_reads(&original));
    effects.assert_read_only();
    assert_calls(&fixture, &ready_reads(&original));
}

#[tokio::test]
async fn failed_or_malformed_reads_never_become_success_or_name_discovery() {
    for content in [false, true] {
        for code in [403, 404, 500, 200] {
            let fixture = Fixture::new().await;
            let original = observation(true);
            install(&fixture, &original);
            let path = if content {
                CONTENT.into()
            } else {
                snapshot_path(&original.request)
            };
            let (status, body) = if code == 200 {
                (200, json!({"metadata":42}))
            } else {
                failure(code)
            };
            fixture
                .state
                .lock()
                .unwrap()
                .read_replies
                .push_back((path, status, body));
            assert!(
                fixture
                    .client
                    .observe_native_home_snapshot(&original)
                    .await
                    .is_err(),
                "content={content}, code={code}"
            );
            let mut expected = ready_reads(&original);
            if !content {
                expected.truncate(1);
            }
            assert_calls(&fixture, &expected);
        }
    }
}

#[tokio::test]
async fn cancelled_read_and_recreated_adapter_keep_the_original_observation() {
    for replacement in [false, true] {
        let fixture = Fixture::new().await;
        let original = observation(true);
        install(&fixture, &original);
        let gate = Gate::new();
        let effects = Effects::new(&fixture, BTreeMap::from([(CONTENT.into(), gate.clone())]));
        let pools = effects.pools(&fixture);
        let previous = original.clone();
        let read = tokio::spawn(async move { pools.observe_native_home_snapshot(&previous).await });
        gate.wait().await;
        read.abort();
        assert!(read.await.unwrap_err().is_cancelled());
        gate.respond();
        if replacement {
            fixture.state.lock().unwrap().rows.get_mut(CONTENT).unwrap()["status"]
                ["snapshotHandle"] = json!("replacement-copy");
        }
        let recreated = effects.pools(&fixture);
        let outcome = recreated.observe_native_home_snapshot(&original).await;
        if replacement {
            assert!(outcome.is_err());
        } else {
            assert_eq!(outcome.unwrap(), original);
        }
        let mut expected = ready_reads(&original);
        expected.extend(ready_reads(&original));
        assert_eq!(*effects.calls.lock().unwrap(), expected);
        effects.assert_read_only();
    }
}

#[tokio::test]
async fn unknown_capture_stays_pending_until_a_real_cut_and_handle_are_observed() {
    let fixture = Fixture::new().await;
    let pending = observation(false);
    install(&fixture, &pending);
    fixture.state.lock().unwrap().rows.insert(
        PVC.into(),
        json!({
            "apiVersion":"v1", "kind":"PersistentVolumeClaim",
            "metadata":{"name":"home","namespace":"cma-test","uid":"pvc-uid-1"}
        }),
    );
    assert_eq!(
        fixture
            .client
            .observe_native_home_snapshot(&pending)
            .await
            .unwrap(),
        pending
    );
    assert_calls(
        &fixture,
        &[
            format!("GET {PVC}"),
            format!("GET {}", snapshot_path(&pending.request)),
            format!("GET {PVC}"),
        ],
    );
    fixture.state.lock().unwrap().calls.clear();
    let ready = observation(true);
    install(&fixture, &ready);
    assert_eq!(
        fixture
            .client
            .observe_native_home_snapshot(&pending)
            .await
            .unwrap(),
        ready
    );
    assert_calls(
        &fixture,
        &[
            format!("GET {PVC}"),
            format!("GET {}", snapshot_path(&pending.request)),
            format!("GET {CONTENT}"),
            format!("GET {PVC}"),
        ],
    );
    fixture.state.lock().unwrap().rows.remove(PVC);
    fixture.state.lock().unwrap().calls.clear();
    assert_eq!(
        fixture
            .client
            .observe_native_home_snapshot(&ready)
            .await
            .unwrap(),
        ready
    );
    assert_calls(&fixture, &ready_reads(&ready));
}
