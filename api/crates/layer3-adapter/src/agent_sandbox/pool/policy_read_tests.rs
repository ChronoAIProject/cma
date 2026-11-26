use super::{
    fixture::{prefix, Fixture},
    read_fixture::{failure, Gate, Reads},
    tests::spec,
};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

#[tokio::test]
async fn policy_binding_starts_template_and_pool_reads_before_either_response() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
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
    let task = tokio::spawn(async move { pools.bound_resources(&binding).await });
    template_gate.wait().await;
    pool_gate.wait().await;
    assert!(!task.is_finished());
    template_gate.respond();
    pool_gate.respond();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reads.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn policy_binding_preserves_template_identity_and_pool_authority_errors() {
    for change in ["api", "uid", "spec", "deleting"] {
        let fixture = Fixture::new().await;
        let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
        let template_path = format!("{}/sandboxtemplates/{}", prefix(), binding.template_name);
        let pool_path = format!("{}/sandboxwarmpools/{}", prefix(), binding.name);
        let mut template = fixture.row("sandboxtemplates", &binding.template_name);
        match change {
            "uid" => template["metadata"]["uid"] = json!("replacement"),
            "spec" => {
                template["spec"]["podTemplate"]["spec"]["containers"][0]["image"] = json!("changed")
            }
            "deleting" => template["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z"),
            _ => (),
        }
        let template_reply = if change == "api" {
            failure(429)
        } else {
            (200, template)
        };
        let baseline = Reads::new(
            &fixture,
            BTreeMap::from([(template_path.clone(), template_reply.clone())]),
            BTreeMap::new(),
        );
        let expected = baseline
            .pools(&fixture)
            .bound_resources(&binding)
            .await
            .unwrap_err();
        let template_gate = Gate::new();
        let pool_gate = Gate::new();
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([
                (template_path.clone(), template_reply),
                (pool_path.clone(), failure(403)),
            ]),
            BTreeMap::from([
                (template_path, template_gate.clone()),
                (pool_path, pool_gate.clone()),
            ]),
        );
        let pools = reads.pools(&fixture);
        let mut task = tokio::spawn(async move { pools.bound_resources(&binding).await });
        template_gate.wait().await;
        pool_gate.wait().await;
        pool_gate.respond();
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut task)
            .await
            .is_err());
        template_gate.respond();
        let actual = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        if change == "spec" {
            assert!(matches!(
                expected,
                crate::AdapterError::NativePoolConfigurationDrift
            ));
            // The typed hold requires current pool authority. Surface a failed
            // pool read even when the original template's spec has drifted.
            assert_eq!(
                format!("{actual:?}"),
                "Rejected(\"Kubernetes rejected Sandbox operation (403)\")"
            );
        } else {
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "{change}");
        }
        assert_eq!(reads.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn policy_binding_rejects_changed_pool_and_invalid_local_authority_before_writes() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
    let pool_path = format!("{}/sandboxwarmpools/{}", prefix(), binding.name);
    for change in ["api", "uid", "template", "deleting"] {
        let mut pool = fixture.row("sandboxwarmpools", &binding.name);
        match change {
            "uid" => pool["metadata"]["uid"] = json!("replacement"),
            "template" => pool["spec"]["sandboxTemplateRef"]["name"] = json!("other-template"),
            "deleting" => pool["metadata"]["deletionTimestamp"] = json!("2026-10-14T00:00:00Z"),
            _ => (),
        }
        let reply = if change == "api" {
            failure(403)
        } else {
            (200, pool)
        };
        let reads = Reads::new(
            &fixture,
            BTreeMap::from([(pool_path.clone(), reply)]),
            BTreeMap::new(),
        );
        assert!(
            reads
                .pools(&fixture)
                .bound_resources(&binding)
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(reads.calls.lock().unwrap().len(), 2);
    }
    let reads = Reads::new(&fixture, BTreeMap::new(), BTreeMap::new());
    let mut invalid = binding;
    invalid.config_sha256 = "f".repeat(64);
    assert!(reads
        .pools(&fixture)
        .bound_resources(&invalid)
        .await
        .is_err());
    assert!(reads.calls.lock().unwrap().is_empty());
}
