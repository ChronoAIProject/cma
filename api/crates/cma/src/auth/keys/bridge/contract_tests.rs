use super::*;
use serde_json::json;

fn owner() -> ResourceOwner {
    ResourceOwner::Organization {
        subject: SubjectId::new("org-one"),
    }
}
fn actor() -> SubjectId {
    SubjectId::new("admin-one")
}
fn scope() -> serde_json::Value {
    json!({"authority":"nyxid","contract_version":"1","policy_version":"api-key-scope-v1",
        "authenticated_actor":{"type":"personal","id":"admin-one"},"intended_key_owner":{"type":"organization","id":"org-one"},
        "services":[{"user_service_id":"service-one","resource_owner":{"type":"organization","id":"org-one"},"node_grant":{"type":"required","node_ids":["node-one"]}}],
        "allowed_service_ids":["service-one"],"allowed_node_ids":["node-one"],"allow_auto_connected_services":false,"normalized_grant_digest":format!("sha256:{}", "a".repeat(64)),
        "completeness":{"list_complete":true,"no_duplicates":true,"route_candidate_basis":"active_configured_routes","transient_node_state_excluded":true},
        "freshness":{"mode":"mutation_revalidated_snapshot","precondition_field":"scope_plan_digest","post_creation_drift":"fail_closed"},"key_purpose":"general"})
}

#[test]
fn org_key_service_selection_cannot_use_same_slug_personal_other_org_or_viewer_connection() {
    let rows: ServiceList = serde_json::from_value(json!({"services":[
        {"id":"personal-service","slug":"models","is_active":true,"credential_source":{"type":"personal"}},
        {"id":"other-service","slug":"models","is_active":true,"credential_source":{"type":"org","org_id":"org-other","role":"admin","allowed":true}},
        {"id":"viewer-service","slug":"models","is_active":true,"credential_source":{"type":"org","org_id":"org-one","role":"viewer","allowed":false}},
        {"id":"service-one","slug":"models","is_active":true,"credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":true}}
    ]})).unwrap();
    assert_eq!(
        select_services(
            &owner(),
            &actor(),
            &rows.services,
            &[ServiceId::new("models")]
        )
        .unwrap(),
        vec!["service-one"]
    );
    assert!(select_services(
        &owner(),
        &actor(),
        &rows.services[..3],
        &[ServiceId::new("models")]
    )
    .is_err());
    let rows: ServiceList = serde_json::from_value(json!({"services":[
        {"id":"service-one","slug":"models","is_active":true,"credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":true}},
        {"id":"service-two","slug":"models","is_active":true,"credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":true}}
    ]})).unwrap();
    assert!(select_services(
        &owner(),
        &actor(),
        &rows.services,
        &[ServiceId::new("models")]
    )
    .is_err());
}

#[test]
fn scope_plan_requires_exact_actor_owner_service_node_union_and_mutation_precondition() {
    let plan: ScopePlan = serde_json::from_value(scope()).unwrap();
    plan.require(&owner(), &actor(), &["service-one".into()])
        .unwrap();
    for pointer in [
        "/authenticated_actor/id",
        "/intended_key_owner/type",
        "/services/0/resource_owner/id",
        "/allowed_node_ids/0",
        "/freshness/precondition_field",
    ] {
        let mut value = scope();
        *value.pointer_mut(pointer).unwrap() = json!("wrong");
        let plan: ScopePlan = serde_json::from_value(value).unwrap();
        assert!(
            plan.require(&owner(), &actor(), &["service-one".into()])
                .is_err(),
            "{pointer}"
        );
    }
    let mut value = scope();
    value["completeness"]["list_complete"] = json!(false);
    let plan: ScopePlan = serde_json::from_value(value).unwrap();
    assert!(plan
        .require(&owner(), &actor(), &["service-one".into()])
        .is_err());
    let mut value = scope();
    value["allow_auto_connected_services"] = json!(true);
    let plan: ScopePlan = serde_json::from_value(value).unwrap();
    assert!(plan
        .require(&owner(), &actor(), &["service-one".into()])
        .is_err());
    let mut value = scope();
    value
        .as_object_mut()
        .unwrap()
        .remove("allow_auto_connected_services");
    assert!(serde_json::from_value::<ScopePlan>(value).is_err());
}

#[test]
fn scope_plan_requires_provider_sha256_digest_without_normalizing_it() {
    for digest in [
        "a".repeat(64),
        format!("sha512:{}", "a".repeat(64)),
        format!("SHA256:{}", "a".repeat(64)),
        format!("sha256:{}", "A".repeat(64)),
        format!("sha256:{}", "a".repeat(63)),
        format!("sha256:{}", "a".repeat(65)),
        format!("sha256:{}", "g".repeat(64)),
        format!(" sha256:{}", "a".repeat(64)),
        format!("sha256:{}\n", "a".repeat(64)),
    ] {
        let mut value = scope();
        value["normalized_grant_digest"] = json!(digest);
        let plan: ScopePlan = serde_json::from_value(value).unwrap();
        assert!(
            plan.require(&owner(), &actor(), &["service-one".into()])
                .is_err(),
            "{digest:?}"
        );
    }
}

#[test]
fn minted_key_needs_exact_owner_expiry_and_scope_metadata_before_delivery() {
    let plan: ScopePlan = serde_json::from_value(scope()).unwrap();
    let expires_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
    let returned: MintedKeyResponse = serde_json::from_value(json!({"id":"key-created","full_key":"disposable-fixture-secret","scopes":"proxy read","state_version":1,"allowed_service_ids":["service-one"],"allowed_node_ids":["node-one"],"allow_all_services":false,"allow_all_nodes":false})).unwrap();
    let material = material(returned, &plan, expires_at).unwrap();
    let metadata = json!({"id":"key-created","name":"cma:test:sbx-one","scopes":"proxy read","state_version":1,"allowed_service_ids":["service-one"],"allowed_node_ids":["node-one"],"allow_all_services":false,"allow_all_nodes":false,"allow_auto_connected_services":false,
        "is_active":true,"expires_at":layer2_core::rfc3339_at(expires_at),"purpose":"general","scheduled_write_enabled":false,
        "credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":true}});
    serde_json::from_value::<KeyMetadata>(metadata.clone())
        .unwrap()
        .require(&owner(), &actor(), "cma:test:sbx-one", &material, &plan)
        .unwrap();
    let mut value = metadata.clone();
    value["allow_auto_connected_services"] = json!(true);
    assert!(serde_json::from_value::<KeyMetadata>(value)
        .unwrap()
        .require(&owner(), &actor(), "cma:test:sbx-one", &material, &plan)
        .is_err());
    let mut value = metadata.clone();
    value
        .as_object_mut()
        .unwrap()
        .remove("allow_auto_connected_services");
    assert!(serde_json::from_value::<KeyMetadata>(value).is_err());
    for (field, added) in [
        ("platform", json!("telegram")),
        ("callback_url", json!("https://callback.example")),
        ("bindings_count", json!(1)),
    ] {
        let mut value = metadata.clone();
        value[field] = added;
        assert!(
            serde_json::from_value::<KeyMetadata>(value)
                .unwrap()
                .require(&owner(), &actor(), "cma:test:sbx-one", &material, &plan)
                .is_err(),
            "{field}"
        );
    }
    for pointer in [
        "/credential_source/org_id",
        "/credential_source/role",
        "/allowed_service_ids/0",
        "/expires_at",
        "/purpose",
    ] {
        let mut value = metadata.clone();
        *value.pointer_mut(pointer).unwrap() = json!("wrong");
        let metadata: KeyMetadata = serde_json::from_value(value).unwrap();
        assert!(
            metadata
                .require(&owner(), &actor(), "cma:test:sbx-one", &material, &plan)
                .is_err(),
            "{pointer}"
        );
    }
}
