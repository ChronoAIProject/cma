use serde_json::{json, Value};

fn document() -> Value {
    serde_json::to_value(crate::rest::openapi::document()).unwrap()
}

#[test]
fn spend_operations_publish_authenticated_success_and_problem_responses() {
    let doc = document();
    for (path, method, statuses) in [
        (
            "/api/v1/admin/spend",
            "get",
            vec!["200", "401", "403", "503"],
        ),
        (
            "/api/v1/admin/spend/observations",
            "post",
            vec!["200", "401", "403", "422", "503"],
        ),
    ] {
        let operation = &doc["paths"][path][method];
        assert!(operation.is_object(), "missing {method} {path}");
        assert_eq!(operation["tags"], json!(["administration"]));
        assert_eq!(operation["security"].as_array().unwrap().len(), 2);
        assert_eq!(
            operation["responses"].as_object().unwrap().len(),
            statuses.len()
        );
        for status in statuses {
            let response = &operation["responses"][status];
            let (media, schema) = if status == "200" {
                ("application/json", "SpendView")
            } else {
                ("application/problem+json", "ProblemDocument")
            };
            assert_eq!(
                response["content"][media]["schema"]["$ref"],
                format!("#/components/schemas/{schema}")
            );
            assert!(!response["description"].as_str().unwrap().is_empty());
        }
    }
    let post = &doc["paths"]["/api/v1/admin/spend/observations"]["post"];
    assert_eq!(post["requestBody"]["required"], true);
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ObservationRequest"
    );
    let unavailable = post["responses"]["503"]["description"].as_str().unwrap();
    assert!(unavailable.contains("model_budget_writes_unavailable"));
    assert!(unavailable.contains("platform_spend_unavailable"));
}

#[test]
fn spend_schemas_preserve_optional_fields_and_observation_bounds() {
    let doc = document();
    let schemas = &doc["components"]["schemas"];
    let request = &schemas["ObservationRequest"];
    assert_eq!(request["additionalProperties"], false);
    assert_eq!(
        request["required"],
        json!(["source", "observed_through", "units"])
    );
    assert_eq!(schemas["ObservationSource"]["enum"], json!(["cloud"]));
    assert_eq!(
        request["properties"]["observed_through"]["format"],
        "date-time"
    );
    assert_eq!(request["properties"]["units"]["minimum"], 0);
    assert_eq!(request["properties"]["units"]["maximum"], (1_u64 << 53) - 1);
    assert_eq!(
        schemas["SpendView"]["required"],
        json!(["metering_enabled", "settings_revision"])
    );
    let ledger_required = schemas["LedgerView"]["required"].as_array().unwrap();
    for optional in ["cloud", "evaluated_at"] {
        assert!(!ledger_required.contains(&json!(optional)));
    }
    assert_eq!(
        schemas["ClosureView"]["properties"]["reason"]["enum"],
        json!(["spend_warning", "spend_stop", "metering_unavailable"])
    );
    for name in ["LevelsView", "LedgerView", "CloudView", "ClosureView"] {
        assert!(schemas[name].is_object(), "missing {name}");
    }
}
