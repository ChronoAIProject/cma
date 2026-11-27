use super::*;

pub fn scope_plan_fixture(actor: &str, request: &Value, nodes: &[&str]) -> Value {
    let owner = request
        .get("target_org_id")
        .and_then(Value::as_str)
        .map(|id| json!({"id":id,"type":"organization"}))
        .unwrap_or_else(|| json!({"id":actor,"type":"personal"}));
    let mut services: Vec<String> =
        serde_json::from_value(request["selected_service_ids"].clone()).unwrap();
    services.sort();
    let required = if services.is_empty() {
        vec![]
    } else {
        nodes.to_vec()
    };
    let mut value = json!({
        "authority":"nyxid","contract_version":"1","policy_version":"api-key-scope-v1",
        "authenticated_actor":{"id":actor,"type":"personal"},"intended_key_owner":owner,
        "services": services.iter().map(|id| json!({
            "user_service_id":id,"resource_owner":owner,
            "node_grant": if required.is_empty() { json!({"type":"not_required"}) }
                else { json!({"type":"required","node_ids":required}) }
        })).collect::<Vec<_>>(),
        "allowed_service_ids":services,"allowed_node_ids":required,
        "evaluated_at":layer2_core::now_rfc3339(),"normalized_grant_digest":"pending",
        "freshness":{"mode":"mutation_revalidated_snapshot","precondition_field":"scope_plan_digest","post_creation_drift":"fail_closed"},
        "completeness":{"list_complete":true,"no_duplicates":true,"route_candidate_basis":"active_configured_routes","transient_node_state_excluded":true},
        "key_purpose":"general"
    });
    let plan: ScopePlan = serde_json::from_value(value.clone()).unwrap();
    value["normalized_grant_digest"] = plan.digest().into();
    value
}

#[test]
fn exact_authoritative_route_plan_includes_all_nodes_and_binds_mutation() {
    let owner = ResourceOwner::Person {
        subject: layer2_core::SubjectId::new("owner"),
    };
    let selected = vec!["service-a".into(), "service-b".into()];
    let value = scope_plan_fixture(
        "owner",
        &json!({"selected_service_ids":selected}),
        &["node-a", "node-b"],
    );
    let plan: ScopePlan = serde_json::from_value(value).unwrap();
    assert!(plan.validate(&selected, &owner).is_some());
    let mut body = json!({"allowed_service_ids": selected, "allow_all_nodes": false});
    plan.apply(&mut body);
    assert_eq!(body["allowed_node_ids"], json!(["node-a", "node-b"]));
    assert_eq!(body["scope_plan_digest"], plan.normalized_grant_digest);
    assert_eq!(body["allow_all_nodes"], false);
}

#[test]
fn incomplete_stale_cross_owner_or_extra_authority_is_refused() {
    let owner = ResourceOwner::Person {
        subject: layer2_core::SubjectId::new("owner"),
    };
    let selected = vec!["service-a".into()];
    let original = scope_plan_fixture(
        "owner",
        &json!({"selected_service_ids":selected}),
        &["node-a"],
    );
    for case in 0..16 {
        let mut value = original.clone();
        match case {
            0 => value["authority"] = "catalogue".into(),
            1 => value["contract_version"] = "2".into(),
            2 => value["intended_key_owner"]["id"] = "someone-else".into(),
            3 => value["authenticated_actor"]["id"] = "someone-else".into(),
            4 => value["services"][0]["resource_owner"]["id"] = "someone-else".into(),
            5 => value["allowed_service_ids"] = json!(["service-a", "unrelated"]),
            6 => value["allowed_node_ids"] = json!(["node-a", "unrelated"]),
            7 => value["allowed_node_ids"] = json!([]),
            8 => value["services"][0]["node_grant"]["node_ids"] = json!(["node-a", "node-a"]),
            9 => value["completeness"]["list_complete"] = false.into(),
            10 => value["completeness"]["transient_node_state_excluded"] = false.into(),
            11 => value["freshness"]["post_creation_drift"] = "allow".into(),
            12 => value["evaluated_at"] = "2020-01-01T00:00:00Z".into(),
            13 => value["normalized_grant_digest"] = format!("sha256:{}", "a".repeat(64)).into(),
            14 => value["key_purpose"] = "durable_operation".into(),
            _ => value["services"] = json!([]),
        }
        let plan: ScopePlan = serde_json::from_value(value).unwrap();
        assert!(
            plan.validate(&selected, &owner).is_none(),
            "accepted case {case}"
        );
    }
}

#[test]
fn minted_node_grants_must_match_the_plan_exactly() {
    let expiry = SystemTime::now() + KEY_LIFETIME;
    for nodes in [
        vec!["node-a"],
        vec![],
        vec!["node-a", "extra"],
        vec!["node-a", "node-a"],
    ] {
        let response: MintedKeyResponse = serde_json::from_value(json!({
            "id":"provider-key","full_key":"fixture-value","scopes":KEY_SCOPES,
            "state_version":1,"allowed_service_ids":["service-a"],"allowed_node_ids":nodes,
            "allow_all_services":false,"allow_all_nodes":false
        }))
        .unwrap();
        let valid = nodes == vec!["node-a"];
        assert_eq!(
            minted_key_from_response(response, &["service-a".into()], &["node-a".into()], expiry)
                .is_some(),
            valid
        );
    }
}
