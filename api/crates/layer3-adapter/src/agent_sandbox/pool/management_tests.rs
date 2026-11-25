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
async fn admitting_pool_observes_current_generation_zero_and_refuses_retiring_binding() {
    let fixture = Fixture::new().await;
    let original = fixture.client.configure(&spec(), 2).await.unwrap().binding;
    let target = super::NativePoolTarget {
        target_id: format!("wp_{}", "a".repeat(64)),
        revision: 1,
        replicas: 2,
    };
    for count in [Some(json!(0)), None] {
        fixture.edit("sandboxwarmpools", &original.name, |row| {
            row["status"] = json!({"observedGeneration":row["metadata"]["generation"]});
            if let Some(count) = count {
                row["status"]["readyReplicas"] = count;
            }
        });
        let observed = fixture
            .client
            .observe_admitting_pool(&original, &target, false)
            .await
            .unwrap();
        assert!(observed.observed);
        assert_eq!(observed.binding, original);
        assert_eq!((observed.desired_replicas, observed.ready_replicas), (2, 0));
    }
    for status in [json!({}), json!({"observedGeneration":0,"readyReplicas":0})] {
        fixture.edit("sandboxwarmpools", &original.name, |row| {
            row["status"] = status
        });
        assert!(
            !fixture
                .client
                .observe_admitting_pool(&original, &target, false)
                .await
                .unwrap()
                .observed
        );
    }
    fixture.edit("sandboxwarmpools", &original.name, |row| {
        row["status"] =
            json!({"observedGeneration":row["metadata"]["generation"],"readyReplicas":"zero"});
    });
    assert!(fixture
        .client
        .observe_admitting_pool(&original, &target, false)
        .await
        .is_err());
    ready(&fixture, &original, 0);
    fixture.edit("sandboxwarmpools", &original.name, |row| {
        row["metadata"]["annotations"][super::retirement::RETIRING] = json!("true");
    });
    assert!(fixture
        .client
        .observe_admitting_pool(&original, &target, false)
        .await
        .is_err());
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

#[tokio::test]
async fn pool_drift_observation_preserves_uid_retirement_and_healthy_plan_authority() {
    let fixture = Fixture::new().await;
    let first = fixture.client.configure(&spec(), 2).await.unwrap().binding;
    let mut another = spec();
    another.cpu_millis += 100;
    let plan = fixture.client.plan(&another).unwrap();
    let target = super::NativePoolTarget {
        target_id: format!("wp_{}", "a".repeat(64)),
        revision: 1,
        replicas: 2,
    };
    let second = fixture.client.configure(&another, 2).await.unwrap().binding;
    ready(&fixture, &second, 2);
    let old = fixture.row("sandboxtemplates", &first.template_name);
    fixture.edit("sandboxtemplates", &first.template_name, |row| {
        row["spec"]["podTemplate"]["spec"]["containers"][0]["image"] = json!("drift");
    });
    assert_eq!(
        fixture.client.inventory().await.unwrap_err(),
        crate::AdapterError::NativePoolConfigurationDrift
    );
    assert_eq!(
        fixture
            .client
            .observe_admitting_pool(&first, &target, true)
            .await
            .unwrap_err(),
        crate::AdapterError::NativePoolConfigurationDrift
    );
    assert_eq!(
        fixture
            .client
            .observe_planned_pool(&plan, &target, false)
            .await
            .unwrap()
            .unwrap()
            .binding,
        second
    );
    let plan_first = fixture.client.plan(&spec()).unwrap();
    // The startup apply would request a newer revision of this exact bound plan.
    let resize = super::NativePoolTarget {
        revision: 2,
        ..target.clone()
    };
    assert_eq!(
        fixture
            .client
            .resize_fenced(&first, &resize)
            .await
            .unwrap_err(),
        crate::AdapterError::NativePoolConfigurationDrift
    );
    assert_eq!(
        fixture
            .client
            .observe_planned_pool(&plan_first, &resize, true)
            .await
            .unwrap_err(),
        crate::AdapterError::NativePoolConfigurationDrift
    );
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["annotations"]["cma/native-target-v1"] =
            json!(serde_json::to_string(&super::NativePoolTarget {
                revision: 3,
                ..resize.clone()
            })
            .unwrap());
    });
    assert!(matches!(
        fixture.client.resize_fenced(&first, &resize).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture
            .client
            .observe_admitting_pool(&first, &resize, true)
            .await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture
            .client
            .observe_planned_pool(&plan_first, &resize, true)
            .await,
        Err(crate::AdapterError::Rejected(_))
    ));
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["annotations"]
            .as_object_mut()
            .unwrap()
            .remove("cma/native-target-v1");
    });
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["annotations"]["cma/native-target-v1"] =
            json!(serde_json::to_string(&resize).unwrap());
    });
    assert_eq!(
        fixture
            .client
            .observe_admitting_pool(&first, &resize, true)
            .await
            .unwrap_err(),
        crate::AdapterError::NativePoolConfigurationDrift
    );
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["annotations"]
            .as_object_mut()
            .unwrap()
            .remove("cma/native-target-v1");
    });
    let original_seed = {
        let mut state = fixture.state.lock().unwrap();
        let seed = state.rows.get_mut(seed_path()).unwrap();
        let original = seed.clone();
        seed["metadata"]["uid"] = json!("replaced-seed");
        original
    };
    assert!(matches!(
        fixture.client.resize(&first, 2).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    assert!(matches!(
        fixture.client.resize_fenced(&first, &resize).await,
        Err(crate::AdapterError::Rejected(_))
    ));
    fixture
        .state
        .lock()
        .unwrap()
        .rows
        .insert(seed_path().into(), original_seed);
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["uid"] = json!("replacement");
    });
    assert!(matches!(
        fixture
            .client
            .observe_admitting_pool(&first, &target, true)
            .await,
        Err(crate::AdapterError::Rejected(_))
    ));
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["uid"] = json!(first.uid);
        row["metadata"]["annotations"][super::retirement::RETIRING] = json!("true");
    });
    assert!(matches!(
        fixture
            .client
            .observe_admitting_pool(&first, &target, true)
            .await,
        Err(crate::AdapterError::Rejected(_))
    ));
    fixture.edit("sandboxwarmpools", &first.name, |row| {
        row["metadata"]["annotations"]
            .as_object_mut()
            .unwrap()
            .remove(super::retirement::RETIRING);
    });
    fixture.edit("sandboxtemplates", &first.template_name, |row| *row = old);
    assert_eq!(
        fixture
            .client
            .observe_admitting_pool(&first, &target, true)
            .await
            .unwrap()
            .binding,
        first
    );
}

#[tokio::test]
async fn recorded_pool_definition_absence_checks_surviving_identity() {
    let fixture = Fixture::new().await;
    let binding = fixture.client.configure(&spec(), 2).await.unwrap().binding;
    let pool = format!(
        "{}/sandboxwarmpools/{}",
        super::fixture::prefix(),
        binding.name
    );
    let template = format!(
        "{}/sandboxtemplates/{}",
        super::fixture::prefix(),
        binding.template_name
    );
    for (missing, surviving) in [(&pool, &template), (&template, &pool)] {
        let original = fixture.state.lock().unwrap().rows.remove(missing).unwrap();
        let expected = crate::AdapterError::NativePoolDefinitionAbsent {
            pool: missing == &pool,
            template: missing == &template,
        };
        assert_eq!(
            fixture.client.bound_resources(&binding).await.unwrap_err(),
            expected
        );
        let old_uid = {
            let mut state = fixture.state.lock().unwrap();
            let object = state.rows.get_mut(surviving).unwrap();
            let uid = object["metadata"]["uid"].clone();
            object["metadata"]["uid"] = json!("replacement");
            uid
        };
        assert!(matches!(
            fixture.client.bound_resources(&binding).await,
            Err(crate::AdapterError::Rejected(_))
        ));
        {
            let mut state = fixture.state.lock().unwrap();
            state.rows.get_mut(surviving).unwrap()["metadata"]["uid"] = old_uid;
            state.rows.insert(missing.clone(), original);
        }
        fixture.client.bound_resources(&binding).await.unwrap();
    }
}
