use super::*;
use crate::agent_sandbox::{pool::read_fixture::Effects, AgentSandboxClient};
use std::{collections::BTreeMap, time::Duration};

#[path = "backend_adoption_boundary_tests.rs"]
mod boundaries;

#[path = "backend_adoption_measurement_tests.rs"]
mod measurements;

fn counts(bound: &Bound) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (call, _) in &bound.fixture.state.lock().unwrap().calls {
        assert!(
            call.starts_with("GET "),
            "adoption must not mutate resources"
        );
        *counts.entry(call.clone()).or_default() += 1;
    }
    counts
}

#[tokio::test]
async fn persisted_home_adoption_removes_discovery_reads_but_preserves_fresh_authority() {
    for mode in [
        HomeVolumeMode::default(),
        HomeVolumeMode::NativeVctEmptyV1,
        HomeVolumeMode::NativeVctFullHomeV2,
    ] {
        for explicit in [false, true] {
            for restart in [false, true] {
                let bound = bound_with_mode(explicit, mode).await;
                let home = adopted(&bound.fixture, &bound.claim).await;
                let control = AgentSandboxBackend::new(
                    client(&bound.fixture, false),
                    bound.transport.clone(),
                );
                let candidate = AgentSandboxBackend::new(
                    client(&bound.fixture, false),
                    bound.transport.clone(),
                );
                let control = if restart { &control } else { &bound.backend };
                let candidate = if restart { &candidate } else { &bound.backend };
                bound.fixture.state.lock().unwrap().calls.clear();
                let expected_observation = control
                    .adopt(&bound.runtime, &bound.request)
                    .await
                    .unwrap()
                    .unwrap();
                let mut expected = counts(&bound);
                if home.request.policy.is_native_vct() {
                    // Each identity check discovers PVC -> member, then rereads
                    // the member after rendering from the fresh graph.
                    *expected.get_mut(&format!("GET {VOLUME}")).unwrap() -= 2;
                    *expected.get_mut(&format!("GET {MEMBER}")).unwrap() -= 4;
                }
                bound.fixture.state.lock().unwrap().calls.clear();
                let actual = candidate
                    .adopt_with_home(&bound.runtime, &bound.request, &home)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(actual, expected_observation);
                assert_eq!(counts(&bound), expected,
                    "persisted home removes only six discovery reads: {mode:?}/{explicit}/{restart}");
                assert_eq!(
                    bound.fixture.state.lock().unwrap().assignments.len(),
                    1 + 2 * usize::from(restart)
                );
                assert_eq!(bound.transport.binds.lock().unwrap().len(), 3);
            }
        }
    }
}

#[tokio::test]
async fn persisted_home_adoption_rejects_forged_identity_before_binding() {
    for change in [
        "receipt",
        "missing_origin",
        "scope",
        "claim_uid",
        "pvc_uid",
        "handle",
        "sandbox",
        "home_identity",
        "generation",
        "mount",
        "image",
    ] {
        let bound = bound(false).await;
        let mut home = adopted(&bound.fixture, &bound.claim).await;
        let mut requested = bound.request.clone();
        let mut handle = bound.runtime.clone();
        match change {
            "receipt" => home.receipt.request_sha256 = "forged".into(),
            "missing_origin" => home.receipt.native_authority = None,
            "scope" => {
                home.request.scope = Some(
                    HomeVolumeScope::from_authority(
                        "native-vct-empty-v1",
                        &["foreign", "cma-test", "cma-test"],
                    )
                    .unwrap(),
                );
                home.receipt
                    .native_authority
                    .as_mut()
                    .unwrap()
                    .claim
                    .request
                    .home = home.request.clone();
                home.receipt.request_sha256 = home.request.computed_sha256();
                requested = request(&home, 1);
            }
            "claim_uid" => {
                home.receipt.native_authority.as_mut().unwrap().claim.uid = "forged".into();
                requested = request(&home, 1);
            }
            "pvc_uid" => {
                home.receipt.storage_generation = "forged".into();
                home.receipt.native_authority.as_mut().unwrap().pvc_uid = "forged".into();
                requested = request(&home, 1);
            }
            "handle" => handle = RuntimeHandle("agent-sandbox:other:member-uid".into()),
            "sandbox" => {
                requested
                    .metadata
                    .insert("cma/sandbox".into(), "foreign".into());
            }
            "home_identity" => {
                requested
                    .metadata
                    .insert("cma/home-identity".into(), "forged".into());
            }
            "generation" => requested = request(&home, 2),
            "mount" => requested.volumes[0].pvc.claim_name = "foreign".into(),
            "image" => requested.image.uri = "foreign-image".into(),
            _ => unreachable!(),
        }
        bound.fixture.state.lock().unwrap().calls.clear();
        assert!(
            bound
                .backend
                .adopt_with_home(&handle, &requested, &home)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            bound.fixture.state.lock().unwrap().assignments.len(),
            1,
            "{change}"
        );
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 1, "{change}");
        let _ = counts(&bound);
    }
}

#[tokio::test]
async fn persisted_home_adoption_refuses_live_graph_and_policy_changes_before_dispatch() {
    for restart in [false, true] {
        for change in [
            "claim",
            "volume",
            "member",
            "request",
            "policy",
            "missing_member",
        ] {
            let mut bound = bound(change == "policy").await;
            let home = adopted(&bound.fixture, &bound.claim).await;
            match change {
                "claim" => bound
                    .fixture
                    .edit("sandboxclaims", &bound.claim.name, |row| {
                        row["metadata"]["uid"] = json!("replacement")
                    }),
                "volume" | "member" => {
                    let path = if change == "volume" { VOLUME } else { MEMBER };
                    bound
                        .fixture
                        .state
                        .lock()
                        .unwrap()
                        .rows
                        .get_mut(path)
                        .unwrap()["metadata"]["uid"] = json!("replacement");
                }
                "request" => {
                    bound.request.env.insert("CHANGED".into(), "value".into());
                }
                "policy" => bound.fixture.edit(
                    "sandboxtemplates",
                    &bound.claim.request.pool.template_name,
                    |row| row["spec"]["networkPolicy"]["egress"] = json!([{}]),
                ),
                "missing_member" => {
                    bound.fixture.state.lock().unwrap().rows.remove(MEMBER);
                }
                _ => unreachable!(),
            }
            let candidate =
                AgentSandboxBackend::new(client(&bound.fixture, false), bound.transport.clone());
            let candidate = if restart { &candidate } else { &bound.backend };
            assert!(
                candidate
                    .adopt_with_home(&bound.runtime, &bound.request, &home)
                    .await
                    .is_err(),
                "{change}"
            );
            assert_eq!(
                bound.fixture.state.lock().unwrap().assignments.len(),
                1,
                "{change}"
            );
            assert_eq!(bound.transport.binds.lock().unwrap().len(), 1, "{change}");
        }
    }
}

#[tokio::test]
async fn persisted_home_adoption_rechecks_bootstrap_effect_before_binding() {
    for path in [VOLUME, MEMBER] {
        let bound = bound(true).await;
        let home = adopted(&bound.fixture, &bound.claim).await;
        bound.fixture.state.lock().unwrap().exec_mutations.push((
            path.into(),
            "/metadata/uid".into(),
            json!("replaced-during-bootstrap"),
        ));
        let candidate =
            AgentSandboxBackend::new(client(&bound.fixture, false), bound.transport.clone());
        assert!(candidate
            .adopt_with_home(&bound.runtime, &bound.request, &home)
            .await
            .is_err());
        assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 2);
        assert_eq!(bound.transport.binds.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn persisted_home_adoption_container_restart_requires_assignment_again() {
    let bound = bound(false).await;
    let home = adopted(&bound.fixture, &bound.claim).await;
    bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(POD)
        .unwrap()["status"]["containerStatuses"][0]["containerID"] = json!("containerd://new");
    for _ in 0..2 {
        bound
            .backend
            .adopt_with_home(&bound.runtime, &bound.request, &home)
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(bound.fixture.state.lock().unwrap().assignments.len(), 2);
    assert_eq!(bound.transport.binds.lock().unwrap().len(), 3);
}
