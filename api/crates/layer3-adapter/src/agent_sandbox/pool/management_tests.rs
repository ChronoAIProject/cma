use super::{
    fixture::{seed_path, Fixture},
    tests::spec,
    *,
};
use serde_json::json;

fn ready(fixture: &Fixture, binding: &NativePoolBinding, count: u32) {
    fixture.edit("sandboxwarmpools", &binding.name, |row| {
        row["status"] =
            json!({"observedGeneration":row["metadata"]["generation"],"readyReplicas":count});
    });
}

#[tokio::test]
async fn planned_creation_keeps_pod_policy_after_lost_ack_and_rejects_other_scope() {
    let fixture = Fixture::new().await;
    let plan = fixture.client.plan(&spec()).unwrap();
    let first = fixture.client.configure_planned(&plan, 0).await.unwrap();
    let mut restarted = fixture.client.clone();
    restarted.config.service_account = "new-runtime".into();
    assert_eq!(restarted.configure_planned(&plan, 0).await.unwrap(), first);
    let mut foreign = plan.clone();
    foreign.namespace = "another-namespace".into();
    assert!(restarted.configure_planned(&foreign, 1).await.is_err());
    let mut changed = plan;
    changed.pod_policy.service_account = "unrecorded".into();
    assert!(restarted.configure_planned(&changed, 1).await.is_err());
    assert_eq!(fixture.client.inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn inventory_reconstructs_exact_bindings_and_selection_requires_current_ready_policy() {
    let fixture = Fixture::new().await;
    assert!(fixture.client.inventory().await.unwrap().is_empty());
    let pool = fixture.client.configure(&spec(), 2).await.unwrap();
    assert_eq!(
        fixture.client.inventory().await.unwrap(),
        vec![pool.clone()]
    );
    assert!(fixture.client.select(&spec()).await.unwrap().is_none());
    ready(&fixture, &pool.binding, 2);
    assert_eq!(
        fixture.client.select(&spec()).await.unwrap(),
        Some(pool.binding.clone())
    );
    let mut changed = spec();
    changed.cpu_millis += 1;
    assert!(fixture.client.select(&changed).await.unwrap().is_none());
    let mut client = fixture.client.clone();
    client.config.service_account = "new-service-account".into();
    client.config.runtime_class = Some("new-runtime-class".into());
    client.config.image_pull_secrets = vec!["new-pull-secret".into()];
    assert_eq!(client.inventory().await.unwrap()[0].binding, pool.binding);
    assert!(client.select(&spec()).await.unwrap().is_none());
    // The original binding remains operable after deployment defaults change.
    fixture.lost("PATCH", "sandboxwarmpools");
    let scaled = client.resize(&pool.binding, 3).await.unwrap();
    assert_eq!(scaled.binding, pool.binding);
    assert_eq!(scaled.desired_replicas, 3);
    assert!(!scaled.observed);
    fixture.state.lock().unwrap().rows.remove(seed_path());
    assert_eq!(
        client
            .resize(&pool.binding, 0)
            .await
            .unwrap()
            .desired_replicas,
        0
    );
    assert!(client.resize(&pool.binding, 1).await.is_err());
    // Stale readiness cannot select a deliberately disabled pool.
    ready(&fixture, &pool.binding, 2);
    assert!(fixture.client.select(&spec()).await.unwrap().is_none());
}

#[tokio::test]
async fn resize_never_recreates_missing_or_replaced_pool_or_template() {
    for change in [
        "pool_missing",
        "template_missing",
        "pool_uid",
        "template_uid",
        "binding_digest",
        "binding_name",
    ] {
        let fixture = Fixture::new().await;
        let mut binding = fixture.client.configure(&spec(), 2).await.unwrap().binding;
        match change {
            "pool_missing" | "template_missing" => {
                let (kind, name) = if change == "pool_missing" {
                    ("sandboxwarmpools", &binding.name)
                } else {
                    ("sandboxtemplates", &binding.template_name)
                };
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .rows
                    .remove(&format!("{}/{kind}/{name}", fixture::prefix()));
            }
            "pool_uid" => fixture.edit("sandboxwarmpools", &binding.name, |v| {
                v["metadata"]["uid"] = json!("replacement")
            }),
            "template_uid" => fixture.edit("sandboxtemplates", &binding.template_name, |v| {
                v["metadata"]["uid"] = json!("replacement")
            }),
            "binding_digest" => binding.config_sha256 = "f".repeat(64),
            _ => binding.name = "another-pool".into(),
        }
        let before = fixture.state.lock().unwrap().calls.len();
        assert!(
            fixture.client.resize(&binding, 0).await.is_err(),
            "{change}"
        );
        assert!(fixture.state.lock().unwrap().calls[before..]
            .iter()
            .all(|(call, _)| call.starts_with("GET ")));
    }
}

#[tokio::test]
async fn discovery_refuses_metadata_spec_readiness_or_template_identity_drift() {
    for change in [
        "definition",
        "definition_unknown",
        "digest",
        "owner",
        "namespace",
        "template_uid",
        "image",
        "target",
        "ready",
    ] {
        let fixture = Fixture::new().await;
        let pool = fixture.client.configure(&spec(), 2).await.unwrap();
        ready(&fixture, &pool.binding, 2);
        let mut row = fixture.row("sandboxwarmpools", &pool.binding.name);
        match change {
            "definition" => row["metadata"]["annotations"][DEFINITION_ANNOTATION] = json!("{}"),
            "definition_unknown" => {
                let mut def: serde_json::Value = serde_json::from_str(
                    row["metadata"]["annotations"][DEFINITION_ANNOTATION]
                        .as_str()
                        .unwrap(),
                )
                .unwrap();
                def["extra"] = json!(true);
                row["metadata"]["annotations"][DEFINITION_ANNOTATION] = json!(def.to_string());
            }
            "digest" => row["metadata"]["annotations"][DIGEST_ANNOTATION] = json!("f".repeat(64)),
            "owner" => row["metadata"]["labels"][OWNER_LABEL] = json!("foreign"),
            "namespace" => row["metadata"]["namespace"] = json!("foreign"),
            "template_uid" => fixture.edit("sandboxtemplates", &pool.binding.template_name, |v| {
                v["metadata"]["uid"] = json!("replacement")
            }),
            "image" => fixture.edit("sandboxtemplates", &pool.binding.template_name, |v| {
                v["spec"]["podTemplate"]["spec"]["containers"][0]["image"] = json!("foreign:tag")
            }),
            "target" => row["spec"]["replicas"] = json!(1001),
            _ => row["status"]["readyReplicas"] = json!(-1),
        }
        fixture
            .state
            .lock()
            .unwrap()
            .list_replies
            .push_back((200, page(vec![row], None, "1")));
        assert!(fixture.client.inventory().await.is_err(), "{change}");
    }
}

fn page(items: Vec<serde_json::Value>, cursor: Option<&str>, version: &str) -> serde_json::Value {
    json!({"apiVersion":API_VERSION,"kind":"SandboxWarmPoolList",
        "metadata":{"resourceVersion":version,"continue":cursor},"items":items})
}

#[tokio::test]
async fn inventory_requires_complete_consistent_nonduplicating_bounded_pages() {
    for failure in [
        "none",
        "error",
        "cycle",
        "version",
        "duplicate",
        "missing_version",
        "bound",
    ] {
        let fixture = Fixture::new().await;
        let binding = fixture.client.configure(&spec(), 1).await.unwrap().binding;
        let row = fixture.row("sandboxwarmpools", &binding.name);
        let mut pages = vec![(200, page(vec![row.clone()], Some("next"), "1"))];
        match failure {
            "none" => pages.push((200, page(vec![], None, "1"))),
            "error" => pages.push((500, json!({"apiVersion":"v1","kind":"Status","status":"Failure","reason":"InternalError","message":"fixture","code":500}))),
            "cycle" => pages.push((200, page(vec![], Some("next"), "1"))),
            "version" => pages.push((200, page(vec![], None, "2"))),
            "duplicate" => pages.push((200, page(vec![row], None, "1"))),
            "missing_version" => pages.push((200, page(vec![], None, ""))),
            _ => for n in 0..100 { pages.push((200, page(vec![], Some(&format!("next-{n}")), "1"))); },
        }
        fixture.state.lock().unwrap().list_replies.extend(pages);
        let result = fixture.client.inventory().await;
        if failure == "none" {
            assert_eq!(result.unwrap()[0].binding, binding);
        } else {
            assert!(result.is_err(), "{failure}");
        }
        let state = fixture.state.lock().unwrap();
        assert!(state
            .calls
            .iter()
            .filter(|(call, _)| call.ends_with("/sandboxwarmpools"))
            .all(|(call, _)| call.starts_with("GET ") || call.starts_with("POST ")));
        assert!(state.list_queries[1].contains("continue=next"));
    }
}
