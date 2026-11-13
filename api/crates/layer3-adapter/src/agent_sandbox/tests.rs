use super::*;
use crate::provider::{
    ProviderImage, ProviderPersistentVolumeClaim, ProviderSandboxRequest, ProviderVolume,
};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn kubernetes_resource_version_conflict_is_retryable_but_other_refusals_are_not() {
    let error = |code, reason: &str| {
        api_error(kube::Error::Api(
            serde_json::from_value(json!({
                "status": "Failure", "message": "fixture refusal", "reason": reason, "code": code
            }))
            .unwrap(),
        ))
    };
    assert!(matches!(
        error(409, "Conflict"),
        AdapterError::Unavailable(_)
    ));
    for (code, reason) in [
        (409, "AlreadyExists"),
        (409, ""),
        (403, "Forbidden"),
        (422, "Invalid"),
    ] {
        assert!(matches!(error(code, reason), AdapterError::Rejected(_)));
    }
}

pub(super) fn config() -> AgentSandboxConfig {
    AgentSandboxConfig {
        namespace: "cma-test".into(),
        deployment: "cma-test".into(),
        runtime_class: Some("gvisor".into()),
        service_account: "cma-runtime".into(),
        image_pull_secrets: vec!["registry-pull".into()],
        template_network_policy: None,
        template_fs_group_change_policy: None,
        native_claim_guard_enabled: false,
        control_key: "fixture".repeat(8),
    }
}
pub(super) fn request() -> ProviderSandboxRequest {
    ProviderSandboxRequest {
        funded_until: None,
        image: ProviderImage {
            uri: format!("registry.example/cma@sha256:{}", "a".repeat(64)),
        },
        entrypoint: vec!["/usr/local/bin/cma-runtime".into()],
        env: BTreeMap::new(),
        resource_limits: BTreeMap::from([
            ("cpu".into(), "750m".into()),
            ("memory".into(), "1536Mi".into()),
        ]),
        timeout: None,
        metadata: BTreeMap::from([
            (OWNER_LABEL.into(), "cma-test".into()),
            ("cma/sandbox".into(), "logical-sandbox".into()),
            ("cma/runtime-generation".into(), "2".into()),
        ]),
        volumes: vec![ProviderVolume {
            name: "home".into(),
            pvc: ProviderPersistentVolumeClaim {
                claim_name: "cma-home-test".into(),
                create_if_not_exists: false,
                delete_on_sandbox_termination: false,
            },
            mount_path: "/home/cma".into(),
            read_only: false,
            sub_path: None,
        }],
    }
}

#[test]
fn renders_user_resources_and_pvc_without_cluster_credentials() {
    let object = model::desired(&config(), &request(), "runtime-control").unwrap();
    let pod = &object.data["spec"]["podTemplate"]["spec"];
    assert_eq!(pod["containers"][0]["resources"]["requests"]["cpu"], "750m");
    assert_eq!(
        pod["containers"][0]["resources"]["limits"]["memory"],
        "1536Mi"
    );
    assert_eq!(
        pod["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "cma-home-test"
    );
    assert_eq!(
        pod["containers"][0]["volumeMounts"][0]["mountPath"],
        "/home/cma"
    );
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["runtimeClassName"], "gvisor");
    assert!(!serde_json::to_string(&object)
        .unwrap()
        .contains(&config().control_key));
    let mut other = request();
    other
        .metadata
        .insert(OWNER_LABEL.into(), "another-controller".into());
    assert!(model::desired(&config(), &other, "control").is_err());
    other = request();
    other.image.uri = "registry.example/cma:latest".into();
    assert!(model::desired(&config(), &other, "control").is_err());
}

#[test]
fn creation_names_are_retry_stable_and_generation_specific() {
    let first = model::name(&request()).unwrap();
    assert_eq!(first, model::name(&request()).unwrap());
    let mut replacement = request();
    replacement
        .metadata
        .insert("cma/runtime-generation".into(), "3".into());
    assert_ne!(first, model::name(&replacement).unwrap());
    let mut changed = request();
    changed.resource_limits.insert("cpu".into(), "1000m".into());
    // A changed spec under the same admission identity must conflict, not
    // allocate a second object. The API-side spec comparison rejects it.
    assert_eq!(first, model::name(&changed).unwrap());
}

#[test]
fn same_namespace_or_spoofed_labels_do_not_establish_pod_ownership() {
    let mut pod: Pod = serde_json::from_value(json!({"metadata":{"name":"runtime","namespace":"cma-test",
        "labels":{OWNER_LABEL:"cma-test",pods::TRACKING_LABEL:pods::name_hash("sandbox-one")},
        "ownerReferences":[{"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox","name":"sandbox-one","uid":"original-uid","controller":true}]}})).unwrap();
    assert!(model::owned_pod(
        &pod,
        &config(),
        "sandbox-one",
        "original-uid"
    ));
    assert!(!model::owned_pod(
        &pod,
        &config(),
        "sandbox-one",
        "replacement-uid"
    ));
    pod.metadata.owner_references = None;
    assert!(!model::owned_pod(
        &pod,
        &config(),
        "sandbox-one",
        "original-uid"
    ));
}

#[tokio::test]
async fn kubernetes_inventory_filters_ownership_and_replaced_uid_never_deletes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for i in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 16384];
            let n = socket.read(&mut buffer).await.unwrap();
            let text = String::from_utf8(buffer[..n].to_vec()).unwrap();
            requests.push(text);
            let owned = json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"Sandbox",
                "metadata":{"name":"cma-one","namespace":"cma-test","uid":"new-uid","resourceVersion":"22","labels":{OWNER_LABEL:"cma-test"}}});
            let body = if i == 0 {
                json!({"apiVersion":"agents.x-k8s.io/v1beta1","kind":"SandboxList","metadata":{"resourceVersion":"23"},"items":[owned,
                {"metadata":{"name":"foreign","namespace":"cma-test","labels":{OWNER_LABEL:"foreign-team"}}}]})
            } else {
                owned
            };
            let body = body.to_string();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    let connection = kube::Config::new(format!("http://{address}").parse().unwrap());
    let client = AgentSandboxClient::new(
        kube::Client::try_from(connection).unwrap(),
        config(),
        "fixture-cluster",
    )
    .unwrap();
    assert_eq!(client.inventory().await.unwrap().len(), 1);
    assert!(client
        .delete(&RuntimeHandle("agent-sandbox:cma-one:old-uid".into()))
        .await
        .is_err());
    let requests = server.await.unwrap();
    assert!(requests[0].contains("labelSelector=cma%2Fcontrol-plane%3Dcma-test"));
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}
