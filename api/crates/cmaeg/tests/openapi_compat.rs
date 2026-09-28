mod compat;
use serde_json::{json, Value};

fn snapshot() -> Value {
    serde_json::from_str(include_str!("../openapi/v1.json")).unwrap()
}

#[test]
fn openapi_compat_preserves_the_reviewed_v1_contract() {
    let current = serde_json::from_str(&cmaeg::openapi::snapshot()).unwrap();
    let errors = compat::errors(&snapshot(), &current);
    assert!(
        errors.is_empty(),
        "Non-additive v1 changes:\n{}",
        errors.join("\n")
    );
}

#[test]
fn openapi_compat_export_is_fresh_sorted_and_needs_no_secrets() {
    let generated = cmaeg::openapi::snapshot();
    assert_eq!(
        include_str!("../openapi/v1.json"),
        generated,
        "Regenerate the OpenAPI snapshot, review the diff, and commit it"
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .arg("--print-openapi")
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, generated.as_bytes());
    let document = snapshot();
    assert_eq!(document["info"]["version"], "1.0");
    assert_eq!(document["servers"], json!([{"url":"/v1"}]));
    let parameters = document["paths"]["/channels/{channel_id}"]["delete"]["parameters"]
        .as_array()
        .unwrap();
    let version = parameters
        .iter()
        .find(|parameter| parameter["name"] == "expected_version")
        .expect("channel deletion must require an expected version");
    assert_eq!(version["in"], "query");
    assert_eq!(version["required"], true);
}

#[test]
fn openapi_compat_allows_optional_fields_operations_and_enum_additions() {
    let current = snapshot();
    let mut old = current.clone();
    old["components"]["schemas"]["ContextConfig"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("skills");
    old["components"]["schemas"]["ThinkingEffort"]["enum"]
        .as_array_mut()
        .unwrap()
        .pop();
    old["paths"]
        .as_object_mut()
        .unwrap()
        .remove("/channels/{channel_id}/test");
    assert_eq!(compat::errors(&old, &current), Vec::<String>::new());
}

#[test]
fn openapi_compat_reports_nested_removals_types_and_required_fields() {
    let old = snapshot();
    let mut current = old.clone();
    let schemas = &mut current["components"]["schemas"];
    schemas["ContextConfig"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("skills");
    schemas["ThinkingEffort"]["enum"]
        .as_array_mut()
        .unwrap()
        .pop();
    schemas["Field"]["properties"]["value"]["type"] = json!("integer");
    schemas["ContextConfig"]["required"] = json!(["system_prompt", "new_property"]);
    schemas["PublicSecrets"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("agent_key_hash");
    let errors = compat::errors(&old, &current).join("\n");
    for expected in [
        ".context.skills: removed property",
        "removed enum value \"very_high\"",
        ".value: changed type",
        "new required request property \"system_prompt\"",
        "new required request property \"new_property\"",
        ".secrets.agent_key_hash: removed property",
    ] {
        assert!(errors.contains(expected), "missing {expected}:\n{errors}");
    }
}

#[test]
fn openapi_compat_follows_arrays_and_tagged_variants_and_allows_ref_renames() {
    let old = snapshot();
    let mut current = old.clone();
    let schema = current["components"]["schemas"]
        .as_object_mut()
        .unwrap()
        .remove("Field")
        .unwrap();
    current["components"]["schemas"]["RenamedField"] = schema;
    current["components"]["schemas"]["Block"]["oneOf"][1]["properties"]["fields"]["items"]
        ["$ref"] = json!("#/components/schemas/RenamedField");
    assert!(compat::errors(&old, &current).is_empty());
    current["components"]["schemas"]["Block"]["oneOf"]
        .as_array_mut()
        .unwrap()
        .remove(1);
    let errors = compat::errors(&old, &current).join("\n");
    assert!(errors.contains("removed enum value \"card\""), "{errors}");
}

#[test]
fn openapi_compat_protects_operations_media_security_and_parameters() {
    let old = snapshot();
    let mut current = old.clone();
    let paths = &mut current["paths"];
    paths
        .as_object_mut()
        .unwrap()
        .remove("/events/{event_ref}/typing");
    paths["/channels/{channel_id}"]
        .as_object_mut()
        .unwrap()
        .remove("put");
    paths["/channels"]["post"]["operationId"] = json!("renamed");
    paths["/channels"]["post"]["security"] = json!([]);
    paths["/channels"]["post"]["requestBody"]["content"] = json!({});
    let parameters = paths["/channels/{channel_id}"]["delete"]["parameters"]
        .as_array_mut()
        .unwrap();
    let version = parameters
        .iter_mut()
        .find(|parameter| parameter["name"] == "expected_version")
        .unwrap();
    version["in"] = json!("header");
    paths["/channels/{channel_id}"]["get"]["responses"]
        .as_object_mut()
        .unwrap()
        .remove("200");
    let errors = compat::errors(&old, &current).join("\n");
    for expected in [
        "removed path",
        "removed method",
        "changed operationId",
        "changed security",
        "removed media type",
        "removed parameter",
        "new required parameter",
        "removed response",
    ] {
        assert!(errors.contains(expected), "missing {expected}:\n{errors}");
    }
}
