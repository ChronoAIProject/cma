use super::*;

#[test]
fn source_control_consent_is_a_separate_authenticated_exact_status_contract() {
    let doc = serde_json::to_value(document()).unwrap();
    let path = &doc["paths"]["/api/v1/bridge-invoke-grants/{id}/source-controls"];
    for method in ["get", "put"] {
        let operation = &path[method];
        assert_eq!(operation["security"].as_array().unwrap().len(), 2);
        assert!(operation["responses"]["200"].is_object());
        for wrong in ["201", "202", "204"] {
            assert!(operation["responses"][wrong].is_null());
        }
        for problem in ["400", "401", "403", "404", "409", "503"] {
            assert!(operation["responses"][problem].is_object());
        }
    }
}
#[test]
fn source_control_consent_has_closed_request_required_absence_and_bounded_revisions() {
    let doc = serde_json::to_value(document()).unwrap();
    let schemas = &doc["components"]["schemas"];
    let request = &schemas["SourceControlRequestShape"];
    assert_eq!(request["additionalProperties"], false);
    let required = request["required"].as_array().unwrap();
    assert_eq!(required.len(), 4);
    for name in [
        "expected_bridge_revision",
        "expected_grant_revision",
        "expected_policy_revision",
        "action",
    ] {
        assert!(required.contains(&serde_json::json!(name)));
    }
    assert_eq!(
        request["properties"]["expected_bridge_revision"]["minimum"],
        1
    );
    assert_eq!(
        request["properties"]["expected_bridge_revision"]["maximum"],
        serde_json::json!(i64::MAX)
    );
    assert_eq!(
        request["properties"]["expected_grant_revision"]["maximum"],
        2
    );
    assert_eq!(
        request["properties"]["expected_policy_revision"]["maximum"],
        255
    );
    let state = &schemas["SourceControlStateShape"];
    assert!(state["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("policy")));
    let policy = state["properties"]["policy"].to_string();
    assert!(policy.contains("null"), "{policy}");
    assert_eq!(
        schemas["SourceControlPolicyShape"]["properties"]["revision"]["maximum"],
        256
    );
    assert_eq!(
        schemas["SourceControlScopeShape"]["enum"],
        serde_json::json!(["source_sender"])
    );
    assert_eq!(
        schemas["SourceControlApprovalModeShape"]["enum"],
        serde_json::json!(["dashboard_only"])
    );
}
