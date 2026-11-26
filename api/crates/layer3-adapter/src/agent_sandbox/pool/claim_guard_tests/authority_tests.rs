use super::{fixture, Fixture, NativePoolClaim, MEMBER, NATIVE_HOME_ANNOTATION};
use crate::{agent_sandbox::NativeClaimGuardReason, AdapterError};
use metrics_exporter_prometheus::PrometheusHandle;
use serde_json::Value;
use std::time::{Duration, Instant};

use super::super::{fixture::prefix, home_tests::VOLUME};

fn metric(handle: &PrometheusHandle, outcome: &str, minimum: Duration) {
    let rendered = handle.render();
    let labels = |line: &&str| {
        line.contains("stage=\"home_authority\"")
            && line.contains(&format!("outcome=\"{outcome}\""))
    };
    for name in [
        "cma_native_claim_guard_total{",
        "cma_native_claim_guard_seconds_count{",
    ] {
        let samples: Vec<_> = rendered
            .lines()
            .filter(|line| line.starts_with(name))
            .filter(labels)
            .collect();
        assert_eq!(samples.len(), 1, "{rendered}");
        assert!(samples[0].ends_with(" 1"), "{rendered}");
    }
    let sum = rendered
        .lines()
        .filter(|line| line.starts_with("cma_native_claim_guard_seconds_sum{"))
        .find(labels)
        .unwrap()
        .split_whitespace()
        .last()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!(sum >= minimum.as_secs_f64(), "{rendered}");
    assert!(sum < 3.0, "{rendered}");
    assert!(!rendered.contains("warm-member"));
}

pub(super) fn published(fixture: &Fixture, claim: &NativePoolClaim) -> Value {
    let state = fixture.state.lock().unwrap();
    let claim_path = format!("{}/sandboxclaims/{}", prefix(), claim.name);
    let claim_row = &state.rows[&claim_path];
    let member = &state.rows[MEMBER];
    let authority = &claim_row["metadata"]["annotations"][NATIVE_HOME_ANNOTATION];
    assert!(authority.is_string());
    assert_eq!(
        member["metadata"]["annotations"][NATIVE_HOME_ANNOTATION],
        *authority
    );
    assert_eq!(claim_row["metadata"]["uid"], claim.uid);
    assert_eq!(member["metadata"]["uid"], "member-uid");
    assert_eq!(state.rows[VOLUME]["metadata"]["uid"], "pvc-uid");
    let writes: Vec<_> = state
        .calls
        .iter()
        .filter(|(call, _)| !call.starts_with("GET "))
        .map(|(call, _)| call.as_str())
        .collect();
    assert_eq!(
        writes,
        [format!("PATCH {claim_path}"), format!("PATCH {MEMBER}")]
    );
    authority.clone()
}

fn acknowledge(fixture: &Fixture) {
    let mut state = fixture.state.lock().unwrap();
    let member = state.rows.get_mut(MEMBER).unwrap();
    member["status"]["conditions"][0]["observedGeneration"] =
        member["metadata"]["generation"].clone();
}

pub(super) async fn retry(fixture: &Fixture, claim: &NativePoolClaim, authority: &Value) {
    acknowledge(fixture);
    let before = fixture.state.lock().unwrap().rows.clone();
    fixture.state.lock().unwrap().calls.clear();
    let home = tokio::time::timeout(Duration::from_secs(3), fixture.client.adopt_home(claim))
        .await
        .unwrap()
        .unwrap();
    let origin = home.receipt.native_authority.as_ref().unwrap();
    assert_eq!(origin.claim, *claim);
    assert_eq!(origin.sandbox_uid, "member-uid");
    assert_eq!(origin.pvc_uid, "pvc-uid");
    assert_eq!(
        serde_json::to_value(serde_json::to_string(origin).unwrap()).unwrap(),
        *authority
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows, before);
    assert!(state.calls.iter().all(|(call, _)| call.starts_with("GET ")));
}

async fn stalled_initial_read(resource: &str) {
    let (fixture, claim) = fixture(true).await;
    let path = match resource {
        "claim" => format!("{}/sandboxclaims/{}", prefix(), claim.name),
        "member" => MEMBER.into(),
        _ => VOLUME.into(),
    };
    let original = fixture.state.lock().unwrap().rows.clone();
    fixture.state.lock().unwrap().calls.clear();
    let gate = fixture.gate(format!("GET {path}"), Some(format!("PATCH {MEMBER}")));
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let (result, before) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(4), fixture.client.adopt_home(&claim)),
        async {
            tokio::time::timeout(Duration::from_secs(4), gate.entered.notified())
                .await
                .expect("fixture did not reach the selected authority response");
            fixture.state.lock().unwrap().rows.clone()
        }
    );
    assert_eq!(
        result
            .expect("first acknowledgement GET escaped its deadline")
            .unwrap_err(),
        AdapterError::NativeClaimNotReady(NativeClaimGuardReason::TimedOut)
    );
    assert_eq!(fixture.state.lock().unwrap().rows, before);
    assert_eq!(before[VOLUME], original[VOLUME]);
    assert_eq!(before[MEMBER]["spec"], original[MEMBER]["spec"]);
    let authority = published(&fixture, &claim);
    metric(&handle, "timeout", Duration::from_millis(1800));
    gate.release.notify_one();
    retry(&fixture, &claim, &authority).await;
}

#[tokio::test]
async fn initial_claim_acknowledgement_stall_is_bounded() {
    stalled_initial_read("claim").await;
}

#[tokio::test]
async fn initial_member_acknowledgement_stall_is_bounded() {
    stalled_initial_read("member").await;
}

#[tokio::test]
async fn initial_pvc_acknowledgement_stall_is_bounded() {
    stalled_initial_read("pvc").await;
}

#[tokio::test]
async fn immediately_ready_authority_records_one_phase() {
    let (fixture, claim) = fixture(true).await;
    fixture.state.lock().unwrap().calls.clear();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    fixture.client.adopt_home(&claim).await.unwrap();
    published(&fixture, &claim);
    metric(&handle, "ready", Duration::ZERO);
}

#[tokio::test]
async fn initial_read_and_pending_polls_share_one_deadline_and_metric() {
    for becomes_ready in [true, false] {
        let (fixture, claim) = fixture(true).await;
        fixture.state.lock().unwrap().native_annotation_generation = true;
        fixture.state.lock().unwrap().calls.clear();
        let gate = fixture.gate(format!("GET {MEMBER}"), Some(format!("PATCH {MEMBER}")));
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let started = Instant::now();
        let (result, ()) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(3), fixture.client.adopt_home(&claim)),
            async {
                tokio::time::timeout(Duration::from_secs(4), gate.entered.notified())
                    .await
                    .expect("fixture did not reach the selected authority response");
                tokio::time::sleep(Duration::from_millis(1200)).await;
                if becomes_ready {
                    acknowledge(&fixture);
                }
                gate.release.notify_one();
            }
        );
        let result = result.expect("initial read time must not reset the poll budget");
        if becomes_ready {
            assert_eq!(result.unwrap().receipt.storage_generation, "pvc-uid");
            metric(&handle, "ready", Duration::from_millis(1400));
        } else {
            let outcome = super::not_ready_outcome(&result.unwrap_err());
            assert!(started.elapsed() < Duration::from_millis(2700));
            metric(&handle, outcome, Duration::from_millis(1800));
        }
        let authority = published(&fixture, &claim);
        retry(&fixture, &claim, &authority).await;
    }
}

#[tokio::test]
async fn cancelled_first_observation_retains_publication_and_records_cancellation() {
    let (fixture, claim) = fixture(true).await;
    fixture.state.lock().unwrap().calls.clear();
    let gate = fixture.gate(format!("GET {MEMBER}"), Some(format!("PATCH {MEMBER}")));
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    {
        let adoption = fixture.client.adopt_home(&claim);
        tokio::pin!(adoption);
        tokio::select! {
            result = &mut adoption => panic!("unexpected completion: {result:?}"),
            () = gate.entered.notified() => {}
        }
    }
    let authority = published(&fixture, &claim);
    metric(&handle, "cancelled", Duration::ZERO);
    gate.release.notify_one();
    retry(&fixture, &claim, &authority).await;
}

#[tokio::test]
async fn authority_publication_acknowledgements_are_outside_the_read_deadline() {
    for member_patch in [false, true] {
        let (fixture, claim) = fixture(true).await;
        fixture.state.lock().unwrap().calls.clear();
        let path = if member_patch {
            MEMBER.into()
        } else {
            format!("{}/sandboxclaims/{}", prefix(), claim.name)
        };
        let gate = fixture.gate(format!("PATCH {path}"), None);
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let (result, ()) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(4), fixture.client.adopt_home(&claim)),
            async {
                tokio::time::timeout(Duration::from_secs(4), gate.entered.notified())
                    .await
                    .expect("fixture did not reach the selected authority response");
                tokio::time::sleep(Duration::from_millis(2200)).await;
                assert!(!handle.render().contains("home_authority"));
                gate.release.notify_one();
            }
        );
        assert_eq!(
            result.unwrap().unwrap().receipt.storage_generation,
            "pvc-uid"
        );
        let authority = published(&fixture, &claim);
        metric(&handle, "ready", Duration::ZERO);
        retry(&fixture, &claim, &authority).await;
    }
}

#[tokio::test]
async fn disabled_policy_keeps_initial_read_and_five_second_pending_wait() {
    for pending in [false, true] {
        let (mut fixture, claim) = fixture(true).await;
        fixture.client.config.native_claim_guard_enabled = false;
        fixture.state.lock().unwrap().native_annotation_generation = pending;
        fixture.state.lock().unwrap().calls.clear();
        let gate = fixture.gate(format!("GET {MEMBER}"), Some(format!("PATCH {MEMBER}")));
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let delay = Duration::from_millis(if pending { 200 } else { 2200 });
        let started = Instant::now();
        let (result, ()) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(7), fixture.client.adopt_home(&claim)),
            async {
                tokio::time::timeout(Duration::from_secs(4), gate.entered.notified())
                    .await
                    .expect("fixture did not reach the selected authority response");
                tokio::time::sleep(delay).await;
                gate.release.notify_one();
            }
        );
        let result = result.unwrap();
        if pending {
            super::not_ready_outcome(&result.unwrap_err());
            assert!(started.elapsed() >= delay + Duration::from_secs(5));
        } else {
            assert_eq!(result.unwrap().receipt.storage_generation, "pvc-uid");
            assert!(started.elapsed() >= delay);
        }
        assert!(!handle.render().contains("home_authority"));
        let authority = published(&fixture, &claim);
        retry(&fixture, &claim, &authority).await;
    }
}
