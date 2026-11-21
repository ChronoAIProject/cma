use super::{
    fixture::Fixture,
    home_tests::{adopted, assigned, MEMBER, VOLUME},
};
use crate::{
    agent_sandbox::AgentSandboxClient, hosted::HostedDeployment, provider::ProviderSandboxRequest,
    HomeVolumeAttachment, SandboxCapacity, SandboxSpec,
};
use serde_json::json;

pub(super) fn client(fixture: &Fixture, changed: bool) -> AgentSandboxClient {
    let mut config = fixture.client.config.clone();
    if changed {
        config.runtime_class = Some("new-default".into());
        config.service_account = "new-service-account".into();
        config.image_pull_secrets = vec!["new-secret".into()];
    }
    AgentSandboxClient::new(fixture.kubernetes.clone(), config, "fixture").unwrap()
}

pub(super) fn request(home: &HomeVolumeAttachment, generation: u64) -> ProviderSandboxRequest {
    request_with_spec(home, generation, &super::tests::spec())
}

pub(super) fn request_with_spec(
    home: &HomeVolumeAttachment,
    generation: u64,
    spec: &super::NativePoolSpec,
) -> ProviderSandboxRequest {
    HostedDeployment {
        image: spec.image.clone(),
        ..HostedDeployment::default()
    }
    .render_runtime(
        &SandboxSpec {
            sandbox_id: home.request.sandbox_id.clone(),
            runtime_image: Some(spec.image.uri.clone()),
            created_by: Some("cma-test".into()),
            home_volume: Some(Box::new(home.clone())),
            capacity: SandboxCapacity {
                cpu_millis: spec.cpu_millis,
                memory_mib: spec.memory_mib,
            },
            ..SandboxSpec::default()
        },
        generation,
    )
    .unwrap()
}

fn resign(request: &mut ProviderSandboxRequest) {
    request.metadata.remove("cma/spec-sha256");
    request.metadata.insert(
        "cma/spec-sha256".into(),
        crate::hosted::provider_request_fingerprint(request).unwrap(),
    );
}

#[tokio::test]
async fn runtime_admission_replays_lost_metadata_write_and_keeps_warm_pod_template() {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    let request = request(&home, 1);
    let before = fixture.state.lock().unwrap().rows[MEMBER]["spec"].clone();
    fixture
        .state
        .lock()
        .unwrap()
        .lose
        .push(format!("PATCH {MEMBER}"));
    assert!(client(&fixture, false)
        .admit_pool_runtime(&claim, &home, &request)
        .await
        .is_err());
    let runtime = client(&fixture, true)
        .admit_pool_runtime(&claim, &home, &request)
        .await
        .unwrap();
    assert_eq!(
        client(&fixture, true)
            .provision_pool_runtime(&request)
            .await
            .unwrap(),
        Some(runtime.clone())
    );
    assert_eq!(runtime.as_str(), "agent-sandbox:warm-member:member-uid");
    client(&fixture, true)
        .validate_request(&runtime, &request)
        .await
        .unwrap()
        .unwrap();
    assert!(
        client(&fixture, true).create(&request).await.is_err(),
        "native home must never cold-create a second Sandbox"
    );
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows[MEMBER]["spec"], before);
    assert_eq!(
        state.rows[MEMBER]["metadata"]["annotations"]["cma/sandbox"],
        home.request.sandbox_id
    );
    assert!(!state.rows[MEMBER]
        .to_string()
        .contains("CHRONO_RUNTIME_CONTROL_TOKEN"));
    assert!(!state.calls.iter().any(|(call, _)| call.contains("/pods")
        || call == &format!("POST {}", MEMBER.rsplit_once('/').unwrap().0)));
    assert_eq!(
        state
            .calls
            .iter()
            .filter(
                |(call, patch)| call == &format!("PATCH {MEMBER}") && patch.get("spec").is_none()
            )
            .count(),
        1
    );
}

#[tokio::test]
async fn native_runtime_resume_advances_authority_on_original_crd_without_changing_template_or_home(
) {
    let fixture = Fixture::new().await;
    let claim = assigned(&fixture).await;
    let home = adopted(&fixture, &claim).await;
    let previous = request(&home, 1);
    let runtime = client(&fixture, false)
        .admit_pool_runtime(&claim, &home, &previous)
        .await
        .unwrap();
    let before = fixture.state.lock().unwrap().rows[MEMBER]["spec"]["podTemplate"].clone();
    // Lost suspension response: a new client resumes the same exact operation.
    fixture
        .state
        .lock()
        .unwrap()
        .lose
        .push(format!("PATCH {MEMBER}"));
    assert!(client(&fixture, false)
        .suspend(&runtime, &previous)
        .await
        .is_err());
    client(&fixture, true)
        .suspend(&runtime, &previous)
        .await
        .unwrap();
    client(&fixture, true)
        .verify_suspended(&runtime, &previous)
        .await
        .unwrap();
    let next = request(&home, 2);
    fixture
        .state
        .lock()
        .unwrap()
        .lose
        .push(format!("PATCH {MEMBER}"));
    assert!(client(&fixture, true)
        .resume(&runtime, &previous, &next)
        .await
        .is_err());
    client(&fixture, true)
        .resume(&runtime, &previous, &next)
        .await
        .unwrap();
    client(&fixture, true)
        .validate_request(&runtime, &next)
        .await
        .unwrap()
        .unwrap();
    assert!(client(&fixture, true)
        .validate_request(&runtime, &previous)
        .await
        .is_err());
    let state = fixture.state.lock().unwrap();
    assert_eq!(state.rows[MEMBER]["spec"]["operatingMode"], "Running");
    assert_eq!(state.rows[MEMBER]["spec"]["podTemplate"], before);
    assert_eq!(state.rows[MEMBER]["metadata"]["uid"], "member-uid");
    assert_eq!(state.rows[VOLUME]["metadata"]["uid"], "pvc-uid");
    assert_eq!(
        state.rows[MEMBER]["metadata"]["annotations"]["cma/runtime-generation"],
        "2"
    );
}

#[tokio::test]
async fn runtime_admission_refuses_changed_claim_home_image_command_mounts_and_environment() {
    for change in [
        "claim",
        "home",
        "image",
        "command",
        "mount",
        "identity",
        "generation",
        "resource",
        "fingerprint",
        "request_token",
        "runtime_contract",
        "foreign_runtime",
    ] {
        let fixture = Fixture::new().await;
        let mut claim = assigned(&fixture).await;
        let mut home = adopted(&fixture, &claim).await;
        let mut request = request(&home, 1);
        match change {
            "claim" => claim.uid = "replacement".into(),
            "home" => home.receipt.storage_generation = "replacement".into(),
            "image" => {
                request.image.uri = format!("registry.example/other@sha256:{}", "b".repeat(64))
            }
            "command" => request.entrypoint = vec!["/bin/sh".into()],
            "mount" => request.volumes[0].sub_path = Some("foreign".into()),
            "identity" => {
                request
                    .env
                    .insert("CHRONO_SANDBOX_ID".into(), "other".into());
            }
            "generation" => {
                request
                    .metadata
                    .insert("cma/runtime-generation".into(), "2".into());
            }
            "resource" => {
                request.resource_limits.insert("cpu".into(), "999m".into());
            }
            "request_token" => {
                request
                    .env
                    .insert("CHRONO_RUNTIME_CONTROL_TOKEN".into(), "f".repeat(64));
            }
            "runtime_contract" => {
                request
                    .env
                    .insert("CHRONO_RUNTIME_CONTRACT".into(), "2".into());
            }
            "foreign_runtime" => {
                fixture.state.lock().unwrap().rows.get_mut(MEMBER).unwrap()["metadata"]
                    ["annotations"]["cma/sandbox"] = json!("someone-else")
            }
            _ => {
                request
                    .env
                    .insert("UNFINGERPRINTED".into(), "changed".into());
            }
        }
        if change != "fingerprint" {
            resign(&mut request);
        }
        let before = fixture.state.lock().unwrap().calls.len();
        assert!(
            client(&fixture, false)
                .admit_pool_runtime(&claim, &home, &request)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            fixture.state.lock().unwrap().calls[before..]
                .iter()
                .all(|(call, _)| call.starts_with("GET ")),
            "{change}"
        );
    }
}

#[tokio::test]
async fn admitted_runtime_rechecks_claim_ownership_and_original_home_before_adoption_or_retirement()
{
    for change in ["claim_uid", "owner", "pvc_uid", "runtime_digest"] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        let home = adopted(&fixture, &claim).await;
        let request = request(&home, 1);
        let handle = client(&fixture, false)
            .admit_pool_runtime(&claim, &home, &request)
            .await
            .unwrap();
        if change == "claim_uid" {
            fixture.edit("sandboxclaims", &claim.name, |row| {
                row["metadata"]["uid"] = json!("replacement")
            });
        } else {
            let mut state = fixture.state.lock().unwrap();
            match change {
                "owner" => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["ownerReferences"][0]["uid"] =
                        json!("other-claim")
                }
                "pvc_uid" => {
                    state.rows.get_mut(VOLUME).unwrap()["metadata"]["uid"] = json!("replacement")
                }
                _ => {
                    state.rows.get_mut(MEMBER).unwrap()["metadata"]["annotations"]
                        [super::RUNTIME_REQUEST_ANNOTATION] = json!("0".repeat(64))
                }
            }
        }
        let before = fixture.state.lock().unwrap().calls.len();
        assert!(
            client(&fixture, true)
                .validate_request(&handle, &request)
                .await
                .is_err(),
            "{change}"
        );
        if matches!(change, "claim_uid" | "owner") {
            assert!(
                client(&fixture, true).delete(&handle).await.is_err(),
                "{change}"
            );
        }
        assert!(fixture.state.lock().unwrap().calls[before..]
            .iter()
            .all(|(call, _)| call.starts_with("GET ")));
    }
}

#[tokio::test]
async fn runtime_retirement_deletes_claim_before_member_and_recovers_lost_delete_responses() {
    for phase in ["claim", "sandbox"] {
        let fixture = Fixture::new().await;
        let claim = assigned(&fixture).await;
        let home = adopted(&fixture, &claim).await;
        let request = request(&home, 1);
        let runtime = client(&fixture, false)
            .admit_pool_runtime(&claim, &home, &request)
            .await
            .unwrap();
        let claim_path = format!("{}/sandboxclaims/{}", super::fixture::prefix(), claim.name);
        let path = if phase == "claim" {
            claim_path.as_str()
        } else {
            MEMBER
        };
        fixture
            .state
            .lock()
            .unwrap()
            .lose
            .push(format!("DELETE {path}"));
        assert!(client(&fixture, false).delete(&runtime).await.is_err());
        client(&fixture, true).delete(&runtime).await.unwrap();
        // Another retry after CRD loss must still resolve original claim authority from the PVC.
        client(&fixture, true).delete(&runtime).await.unwrap();
        let state = fixture.state.lock().unwrap();
        assert!(!state.rows.contains_key(&claim_path));
        assert!(!state.rows.contains_key(MEMBER));
        assert_eq!(state.rows[VOLUME]["metadata"]["uid"], "pvc-uid");
        let deletes: Vec<_> = state
            .calls
            .iter()
            .filter(|(call, _)| call.starts_with("DELETE "))
            .collect();
        assert_eq!(deletes.len(), 2, "{phase}");
        assert_eq!(deletes[0].0, format!("DELETE {claim_path}"));
        assert_eq!(deletes[1].0, format!("DELETE {MEMBER}"));
    }
}
