use super::{
    fixture::{prefix, Fixture},
    home_tests::{adopted, assigned_spec, MEMBER, VOLUME},
    network_policy_tests,
    read_fixture::{failure, Gate, Reads},
    runtime_tests, tests, NativeFsGroupChangePolicy, NativePoolClaim,
};
use crate::{
    agent_sandbox::AgentSandboxClient, provider::ProviderSandboxRequest, AdapterError,
    HomeVolumeAttachment, HomeVolumeMode, HomeVolumeScope,
};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

struct Admission {
    fixture: Fixture,
    claim: NativePoolClaim,
    home: HomeVolumeAttachment,
    request: ProviderSandboxRequest,
}

impl Admission {
    async fn new(mode: HomeVolumeMode) -> Self {
        let mut fixture = Fixture::new().await;
        fixture.client.config.template_network_policy =
            Some(network_policy_tests::policy("10.41.0.0/16"));
        let mut spec = tests::spec();
        spec.home.mode = mode;
        if spec.home.is_native_vct() {
            spec.home.template_name.clear();
            spec.home.template_generation.clear();
            fixture.client.config.template_fs_group_change_policy =
                Some(NativeFsGroupChangePolicy::OnRootMismatch);
        }
        spec.image.labels.insert(
            crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL.into(),
            "2".into(),
        );
        let claim = assigned_spec(&fixture, &spec).await;
        let home = adopted(&fixture, &claim).await;
        let request = runtime_tests::request(&home, 1);
        Self {
            fixture,
            claim,
            home,
            request,
        }
    }

    fn paths(&self) -> [String; 5] {
        [
            format!(
                "{}/sandboxtemplates/{}",
                prefix(),
                self.claim.request.pool.template_name
            ),
            format!(
                "{}/sandboxwarmpools/{}",
                prefix(),
                self.claim.request.pool.name
            ),
            format!("{}/sandboxclaims/{}", prefix(), self.claim.name),
            MEMBER.into(),
            VOLUME.into(),
        ]
    }

    fn client(&self, reads: &Reads) -> AgentSandboxClient {
        AgentSandboxClient::new(
            reads.client.clone(),
            self.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap()
    }
}

#[tokio::test]
async fn native_admission_policy_and_home_reads_start_together_and_stay_fresh() {
    for mode in [
        HomeVolumeMode::NativeVctEmptyV1,
        HomeVolumeMode::NativeVctFullHomeV2,
    ] {
        let admission = Admission::new(mode).await;
        let expected = runtime_tests::client(&admission.fixture, false)
            .admit_pool_runtime(&admission.claim, &admission.home, &admission.request)
            .await
            .unwrap();
        let gates: BTreeMap<_, _> = admission
            .paths()
            .into_iter()
            .map(|p| (p, Gate::new()))
            .collect();
        let reads = Reads::new(&admission.fixture, BTreeMap::new(), gates.clone());
        let client = admission.client(&reads);
        let replay = client.clone();
        let claim = admission.claim.clone();
        let home = admission.home.clone();
        let request = admission.request.clone();
        let task =
            tokio::spawn(async move { client.admit_pool_runtime(&claim, &home, &request).await });
        // Neither policy nor graph can finish until all five reads have started.
        for gate in gates.values() {
            gate.wait().await;
        }
        assert_eq!(reads.calls.lock().unwrap().len(), 5);
        assert!(!task.is_finished());
        for gate in gates.values() {
            gate.respond();
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            expected
        );
        assert_eq!(
            replay
                .admit_pool_runtime(&admission.claim, &admission.home, &admission.request)
                .await
                .unwrap(),
            expected
        );
        let calls = reads.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            10,
            "each admission must repeat both fresh proofs"
        );
        for path in admission.paths() {
            assert_eq!(calls.iter().filter(|call| *call == &path).count(), 2);
        }
    }
}

#[tokio::test]
async fn native_admission_awaits_both_proofs_and_keeps_policy_error_precedence() {
    let admission = Admission::new(HomeVolumeMode::NativeVctFullHomeV2).await;
    let paths = admission.paths();
    for policy_first in [false, true] {
        let gates: BTreeMap<_, _> = paths.iter().map(|p| (p.clone(), Gate::new())).collect();
        let reads = Reads::new(
            &admission.fixture,
            BTreeMap::from([
                (paths[0].clone(), failure(429)),
                (paths[2].clone(), failure(403)),
            ]),
            gates.clone(),
        );
        let client = admission.client(&reads);
        let claim = admission.claim.clone();
        let home = admission.home.clone();
        let request = admission.request.clone();
        let mut task =
            tokio::spawn(async move { client.admit_pool_runtime(&claim, &home, &request).await });
        for gate in gates.values() {
            gate.wait().await;
        }
        for (index, path) in paths.iter().enumerate() {
            if (index < 2) == policy_first {
                gates[path].respond();
            }
        }
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        for (index, path) in paths.iter().enumerate() {
            if (index < 2) != policy_first {
                gates[path].respond();
            }
        }
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            matches!(error, AdapterError::AtCapacity(_)),
            "policy must precede the forbidden graph: {error:?}"
        );
        assert_eq!(reads.calls.lock().unwrap().len(), 5);
    }
}

#[tokio::test]
async fn native_admission_refuses_either_live_proof_without_publishing() {
    let admission = Admission::new(HomeVolumeMode::NativeVctFullHomeV2).await;
    for path in admission.paths() {
        let reads = Reads::new(
            &admission.fixture,
            BTreeMap::from([(path.clone(), failure(403))]),
            BTreeMap::new(),
        );
        assert!(
            matches!(
                admission
                    .client(&reads)
                    .admit_pool_runtime(&admission.claim, &admission.home, &admission.request)
                    .await,
                Err(AdapterError::Rejected(_))
            ),
            "{path}"
        );
        // Reads rejects any write; both independent proofs still observe once.
        assert_eq!(reads.calls.lock().unwrap().len(), 5, "{path}");
    }
}

#[tokio::test]
async fn native_admission_rejects_local_origin_and_request_changes_before_reads() {
    let admission = Admission::new(HomeVolumeMode::NativeVctFullHomeV2).await;
    for change in [
        "original_uid",
        "original_name",
        "assignment",
        "origin",
        "receipt",
        "scope",
        "generation",
        "image",
        "mount",
        "sandbox",
        "home_identity",
    ] {
        let mut claim = admission.claim.clone();
        let mut home = admission.home.clone();
        let mut request = admission.request.clone();
        match change {
            "original_uid" => claim.uid = "different-claim".into(),
            "original_name" => claim.name = "different-claim".into(),
            "assignment" => claim.sandbox_name = Some("different-member".into()),
            "origin" => {
                home.receipt.native_authority.as_mut().unwrap().claim.name =
                    "different-claim".into()
            }
            "receipt" => home.receipt.request_sha256 = "invalid".into(),
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
                claim.request.home = home.request.clone();
                request = runtime_tests::request(&home, 1);
            }
            "generation" => request = runtime_tests::request(&home, 2),
            "image" => request.image.uri = "different-image".into(),
            "mount" => request.volumes[0].pvc.claim_name = "different-home".into(),
            "sandbox" => {
                request
                    .metadata
                    .insert("cma/sandbox".into(), "different-sandbox".into());
            }
            "home_identity" => {
                request
                    .metadata
                    .insert("cma/home-identity".into(), "different-home".into());
            }
            _ => unreachable!(),
        }
        let reads = Reads::new(&admission.fixture, BTreeMap::new(), BTreeMap::new());
        assert!(
            admission
                .client(&reads)
                .admit_pool_runtime(&claim, &home, &request)
                .await
                .is_err(),
            "{change}"
        );
        assert!(reads.calls.lock().unwrap().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn legacy_admission_keeps_policy_before_home_and_generation_validation() {
    let admission = Admission::new(HomeVolumeMode::default()).await;
    let expected = runtime_tests::client(&admission.fixture, false)
        .admit_pool_runtime(&admission.claim, &admission.home, &admission.request)
        .await
        .unwrap();
    let paths = admission.paths();
    let gates: BTreeMap<_, _> = paths[..3]
        .iter()
        .map(|p| (p.clone(), Gate::new()))
        .collect();
    let volume = admission.fixture.state.lock().unwrap().rows[VOLUME].clone();
    let reads = Reads::new(
        &admission.fixture,
        BTreeMap::from([(
            "/api/v1/namespaces/cma-test/persistentvolumeclaims".into(),
            (
                200,
                json!({"apiVersion":"v1","kind":"PersistentVolumeClaimList","metadata":{"resourceVersion":"1"},"items":[volume]}),
            ),
        )]),
        gates.clone(),
    );
    let client = admission.client(&reads);
    let claim = admission.claim.clone();
    let home = admission.home.clone();
    let request = admission.request.clone();
    let task =
        tokio::spawn(async move { client.admit_pool_runtime(&claim, &home, &request).await });
    gates[&paths[0]].wait().await;
    gates[&paths[1]].wait().await;
    assert_eq!(reads.calls.lock().unwrap().len(), 2);
    assert!(!task.is_finished());
    gates[&paths[0]].respond();
    gates[&paths[1]].respond();
    gates[&paths[2]].wait().await;
    gates[&paths[2]].respond();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        expected
    );

    let reads = Reads::new(
        &admission.fixture,
        BTreeMap::from([(paths[0].clone(), failure(429))]),
        BTreeMap::new(),
    );
    let request = runtime_tests::request(&admission.home, 2);
    assert!(matches!(
        admission
            .client(&reads)
            .admit_pool_runtime(&admission.claim, &admission.home, &request)
            .await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(
        reads.calls.lock().unwrap().len(),
        2,
        "legacy policy error must still precede generation validation and home reads"
    );
}
