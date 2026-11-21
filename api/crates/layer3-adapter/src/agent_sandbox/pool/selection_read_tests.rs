//! New native selection resolves only its exact configured immutable pair.
use super::{
    fixture::{prefix, Fixture},
    read_fixture::{failure, Gate, Reads},
    NativePoolSpec,
};
use crate::{HomeVolumeMode, HomeVolumeRequest};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

fn spec() -> NativePoolSpec {
    let mut spec = super::tests::spec();
    spec.home.mode = HomeVolumeMode::NativeVctEmptyV1;
    spec.home.template_name.clear();
    spec.home.template_generation.clear();
    spec
}

fn request(spec: &NativePoolSpec) -> (crate::provider::ProviderSandboxRequest, HomeVolumeRequest) {
    let request = crate::hosted::HostedDeployment {
        image: spec.image.clone(),
        ..Default::default()
    }
    .render(&crate::SandboxSpec {
        sandbox_id: "sbx_one".into(),
        created_by: Some("cma-test".into()),
        capacity: crate::SandboxCapacity {
            cpu_millis: spec.cpu_millis,
            memory_mib: spec.memory_mib,
        },
        ..Default::default()
    })
    .unwrap();
    let home = HomeVolumeRequest {
        volume_id: "home-one".into(),
        sandbox_id: "sbx_one".into(),
        policy: spec.home.clone(),
        scope: None,
    };
    (request, home)
}

#[tokio::test]
async fn native_selection_reads_exact_pair_concurrently_and_ignores_historical_pools() {
    let fixture = Fixture::new().await;
    let spec = spec();
    for cpu in [250, 500, 1000, 2000, 4000, 8000, 16000] {
        let mut old = spec.clone();
        old.cpu_millis = cpu;
        let old = fixture.client.configure(&old, 2).await.unwrap().binding;
        // An unrelated retired template does not authorize this reservation.
        fixture.edit("sandboxtemplates", &old.template_name, |row| {
            row["spec"]["podTemplate"]["spec"]["serviceAccountName"] = json!("retired");
        });
    }
    let binding = fixture.client.configure(&spec, 7).await.unwrap().binding;
    let template_path = format!("{}/sandboxtemplates/{}", prefix(), binding.template_name);
    let pool_path = format!("{}/sandboxwarmpools/{}", prefix(), binding.name);
    let template_gate = Gate::new();
    let pool_gate = Gate::new();
    let reads = Reads::new(
        &fixture,
        BTreeMap::new(),
        BTreeMap::from([
            (template_path, template_gate.clone()),
            (pool_path, pool_gate.clone()),
        ]),
    );
    let pools = reads.pools(&fixture);
    let (request, home) = request(&spec);
    let task =
        tokio::spawn(async move { pools.select_runtime(&request, &spec.image, &home).await });
    template_gate.wait().await;
    pool_gate.wait().await;
    assert!(!task.is_finished());
    template_gate.respond();
    pool_gate.respond();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Some(binding.clone())
    );
    assert_eq!(reads.calls.lock().unwrap().len(), 2);
    assert_eq!(
        fixture.row("sandboxwarmpools", &binding.name)["spec"]["replicas"],
        7
    );
}

#[tokio::test]
async fn native_selection_rejects_changed_current_authority_without_writes() {
    let fixture = Fixture::new().await;
    let spec = spec();
    let binding = fixture.client.configure(&spec, 3).await.unwrap().binding;
    let (request, home) = request(&spec);
    for change in [
        "template_missing",
        "template_uid",
        "template_api",
        "template_spec",
        "template_deleting",
        "pool_api",
        "pool_definition",
        "pool_template",
        "pool_template_uid",
        "pool_owner",
        "pool_digest",
        "pool_deleting",
        "pool_uid_missing",
        "pool_target",
    ] {
        let is_template = change.starts_with("template");
        let (resource, name) = if is_template {
            ("sandboxtemplates", &binding.template_name)
        } else {
            ("sandboxwarmpools", &binding.name)
        };
        let mut row = fixture.row(resource, name);
        match change {
            "template_uid" => row["metadata"]["uid"] = json!("replacement"),
            "template_spec" => {
                row["spec"]["podTemplate"]["spec"]["serviceAccountName"] = json!("other")
            }
            "template_deleting" | "pool_deleting" => {
                row["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z")
            }
            "pool_definition" => {
                row["metadata"]["annotations"][super::DEFINITION_ANNOTATION] = json!("{}")
            }
            "pool_template" => row["spec"]["sandboxTemplateRef"]["name"] = json!("other-template"),
            "pool_template_uid" => {
                row["metadata"]["annotations"]["cma/pool-template-uid"] = json!("replacement")
            }
            "pool_owner" => row["metadata"]["labels"][super::OWNER_LABEL] = json!("other"),
            "pool_digest" => {
                row["metadata"]["annotations"][super::DIGEST_ANNOTATION] = json!("f".repeat(64))
            }
            "pool_uid_missing" => row["metadata"]
                .as_object_mut()
                .unwrap()
                .remove("uid")
                .map(|_| ())
                .unwrap(),
            "pool_target" => row["spec"]["replicas"] = json!(1001),
            _ => (),
        }
        let reply = match change {
            "template_missing" => failure(404),
            "template_api" => failure(429),
            "pool_api" => failure(403),
            _ => (200, row),
        };
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([(format!("{}/{resource}/{name}", prefix()), reply)]),
            BTreeMap::new(),
        );
        let error = reads
            .pools(&fixture)
            .select_runtime(&request, &spec.image, &home)
            .await
            .unwrap_err();
        assert_eq!(reads.calls.lock().unwrap().len(), 2, "{change}: {error:?}");
    }
}

#[tokio::test]
async fn native_selection_keeps_template_error_before_faster_pool_failure() {
    let fixture = Fixture::new().await;
    let spec = spec();
    let binding = fixture.client.configure(&spec, 1).await.unwrap().binding;
    let template_path = format!("{}/sandboxtemplates/{}", prefix(), binding.template_name);
    let pool_path = format!("{}/sandboxwarmpools/{}", prefix(), binding.name);
    let template_gate = Gate::new();
    let pool_gate = Gate::new();
    let reads = Reads::new(
        &fixture,
        BTreeMap::from([
            (template_path.clone(), failure(429)),
            (pool_path.clone(), failure(403)),
        ]),
        BTreeMap::from([
            (template_path, template_gate.clone()),
            (pool_path, pool_gate.clone()),
        ]),
    );
    let pools = reads.pools(&fixture);
    let (request, home) = request(&spec);
    let mut task =
        tokio::spawn(async move { pools.select_runtime(&request, &spec.image, &home).await });
    template_gate.wait().await;
    pool_gate.wait().await;
    pool_gate.respond();
    assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
        .await
        .is_err());
    template_gate.respond();
    let error = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, crate::AdapterError::AtCapacity(_)));
    assert_eq!(reads.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_selection_rejects_invalid_local_request_before_reads() {
    let fixture = Fixture::new().await;
    let spec = spec();
    let (request, home) = request(&spec);
    for change in ["identity", "image", "capacity"] {
        let reads = Reads::new(&fixture, BTreeMap::new(), BTreeMap::new());
        let mut home = home.clone();
        let mut request = request.clone();
        match change {
            "identity" => home.sandbox_id = "sbx_other".into(),
            "image" => request.image.uri = "unadmitted".into(),
            "capacity" => {
                request.resource_limits.insert("cpu".into(), "0m".into());
            }
            _ => unreachable!(),
        }
        assert!(
            reads
                .pools(&fixture)
                .select_runtime(&request, &spec.image, &home)
                .await
                .is_err(),
            "{change}"
        );
        assert!(reads.calls.lock().unwrap().is_empty(), "{change}");
    }
}
