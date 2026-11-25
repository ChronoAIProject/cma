use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::{paths, BodyKind, Corpus, ResponseFixture};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inventory {
    version: u32,
    openapi_operations: usize,
    shared_capacity_problems: BTreeSet<String>,
    operations: BTreeMap<String, Operation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    classification: Classification,
    source: String,
    required_problems: BTreeSet<String>,
    #[serde(default)]
    required_protocol_failures: BTreeSet<String>,
    #[serde(default)]
    opaque_protocol_failures: Option<String>,
    #[serde(default)]
    unsupported_success: Option<String>,
    #[serde(default)]
    non_problem_statuses: BTreeMap<u16, String>,
    #[serde(default)]
    unreachable_problems: BTreeMap<String, String>,
    #[serde(default)]
    unreachable_protocol_failures: BTreeMap<String, String>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Classification {
    Openapi,
    SupplementalAuth,
    SupplementalAsset,
    LegacyGateway,
}

#[test]
fn every_documented_operation_and_named_problem_has_exact_contract_coverage() {
    let document = serde_json::to_value(super::super::openapi::document()).unwrap();
    let openapi: BTreeSet<_> = document["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, methods)| {
            methods
                .as_object()
                .unwrap()
                .keys()
                .filter(|method| {
                    ["get", "post", "put", "patch", "delete"].contains(&method.as_str())
                })
                .map(move |method| format!("{} {path}", method.to_ascii_uppercase()))
        })
        .collect();
    let path = super::directory().with_file_name("public-contract-inventory.json");
    let inventory: Inventory = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(inventory.version, 1);
    let advertised = document.to_string();
    for code in &inventory.shared_capacity_problems {
        assert!(
            advertised.contains(code),
            "OpenAPI must describe shared problem {code}"
        );
    }
    assert_eq!(inventory.openapi_operations, 339);
    assert_eq!(
        openapi.len(),
        inventory.openapi_operations,
        "OpenAPI operation count changed"
    );
    let declared: BTreeSet<_> = inventory
        .operations
        .iter()
        .filter(|(_, entry)| entry.classification == Classification::Openapi)
        .map(|(operation, _)| operation.clone())
        .collect();
    assert_eq!(openapi, declared, "OpenAPI operation inventory differs");
    for (operation, entry) in &inventory.operations {
        if entry.classification == Classification::LegacyGateway {
            assert!(operation
                .split_once(' ')
                .unwrap()
                .1
                .starts_with("/api/v1/gateway/"));
            assert!(
                !openapi.contains(operation),
                "retired Gateway storage must not be advertised by Agent Manager"
            );
        }
    }
    let mut ids = BTreeSet::new();
    let mut covered: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut protocol_failures: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut success = BTreeSet::new();
    for path in paths() {
        let corpus: Corpus = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(corpus.version, 1);
        for case in corpus.cases {
            case.response.expected_headers();
            assert!(ids.insert(case.id.clone()), "duplicate case {}", case.id);
            let entry = inventory
                .operations
                .get(&case.operation)
                .unwrap_or_else(|| panic!("unclassified operation {}", case.operation));
            assert!(
                route_matches(&case.operation, &case.request.method, &case.request.path),
                "case {} does not match its operation",
                case.id
            );
            match outcome(&case.response) {
                Outcome::Success => {
                    success.insert(case.operation.clone());
                }
                Outcome::Problems(codes) => {
                    for code in codes {
                        assert!(
                            entry.required_problems.contains(&code),
                            "{} emitted undeclared problem {code}",
                            case.id
                        );
                        covered
                            .entry(case.operation.clone())
                            .or_default()
                            .insert(code);
                    }
                }
                Outcome::ProtocolFailures(codes) => {
                    for code in codes {
                        assert!(
                            entry.required_protocol_failures.contains(&code),
                            "{} emitted undeclared protocol failure {code}",
                            case.id
                        );
                        protocol_failures
                            .entry(case.operation.clone())
                            .or_default()
                            .insert(code);
                    }
                }
                Outcome::NonProblem => {
                    assert!(
                        entry
                            .non_problem_statuses
                            .contains_key(&case.response.status),
                        "{} requires explicit non-problem status classification",
                        case.id
                    );
                }
            }
        }
    }
    let mut missing = Vec::new();
    for (operation, entry) in &inventory.operations {
        assert_source(&entry.source);
        if !success.contains(operation) && entry.unsupported_success.is_none() {
            missing.push(format!("{operation}: success"));
        }
        if let Some(source) = &entry.unsupported_success {
            assert_source(source);
        }
        if let Some(source) = &entry.opaque_protocol_failures {
            assert_source(source);
        }
        for source in entry
            .non_problem_statuses
            .values()
            .chain(entry.unreachable_problems.values())
            .chain(entry.unreachable_protocol_failures.values())
        {
            assert_source(source);
        }
        for code in &entry.required_problems {
            assert!(
                !entry.unreachable_problems.contains_key(code),
                "{operation} declares both required and unreachable {code}"
            );
            if !covered
                .get(operation)
                .is_some_and(|codes| codes.contains(code))
            {
                missing.push(format!("{operation}: {code}"));
            }
        }
        for code in &entry.required_protocol_failures {
            assert!(
                !entry.unreachable_protocol_failures.contains_key(code),
                "{operation} declares both required and unreachable protocol failure {code}"
            );
            if !protocol_failures
                .get(operation)
                .is_some_and(|codes| codes.contains(code))
            {
                missing.push(format!("{operation}: protocol failure {code}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "missing operation/problem pairs:\n{}",
        missing.join("\n")
    );
    eprintln!(
        "Shared contract: {} cases, {} OpenAPI operations, {} exact named HTTP problem pairs, {} protocol failure pairs",
        ids.len(),
        openapi.len(),
        covered.values().map(BTreeSet::len).sum::<usize>(),
        protocol_failures.values().map(BTreeSet::len).sum::<usize>()
    );
}

#[test]
fn fixed_protocol_outcomes_have_complete_reachability_dispositions() {
    let path = super::directory().with_file_name("public-contract-inventory.json");
    let inventory: Inventory = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let fixed = BTreeSet::from([
        "agent_deleted",
        "agent_turn_failed",
        "failed",
        "input_not_written",
        "operator",
        "owner",
        "provisioning_timeout",
        "response_unavailable",
        "runtime_lost",
        "sandbox_retired",
    ]);
    let mut required = 0;
    for operation in [
        "GET /api/v1/agui/events",
        "POST /api/v1/agui/run",
        "GET /api/v1/responses/{response_id}",
        "POST /api/v1/responses",
    ] {
        let entry = &inventory.operations[operation];
        let classified = entry
            .required_protocol_failures
            .iter()
            .chain(entry.unreachable_protocol_failures.keys())
            .map(String::as_str)
            .collect();
        let mut expected = fixed.clone();
        if operation == "GET /api/v1/agui/events" {
            // Retained discovery also pins this existing persisted capacity class.
            expected.insert("agent_log_capacity_exceeded");
        }
        assert_eq!(
            expected, classified,
            "incomplete fixed outcomes for {operation}"
        );
        required += entry.required_protocol_failures.len();
    }
    assert_eq!(required, 16, "fixed protocol reachability matrix changed");
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Success,
    Problems(BTreeSet<String>),
    ProtocolFailures(BTreeSet<String>),
    NonProblem,
}

fn outcome(response: &ResponseFixture) -> Outcome {
    if response.body_kind == BodyKind::Sse {
        let mut codes = BTreeSet::new();
        for frame in response.body.as_array().expect("SSE body must be frames") {
            let data = &frame["data"];
            let event = frame["event"].as_str().or_else(|| data["type"].as_str());
            let code = match event {
                Some("RUN_ERROR") => data["code"].as_str(),
                Some("response.failed") => data["response"]["error"]["code"].as_str(),
                Some("error") => data["error"]["code"]
                    .as_str()
                    .or_else(|| data["code"].as_str()),
                _ => continue,
            };
            codes.insert(
                code.filter(|code| !code.trim().is_empty())
                    .expect("stream failure requires a named problem code")
                    .to_owned(),
            );
        }
        if !codes.is_empty() {
            return Outcome::ProtocolFailures(codes);
        }
    }
    if response.body["object"] == "response" && response.body["status"] == "failed" {
        let code = response.body["error"]["code"]
            .as_str()
            .filter(|code| !code.trim().is_empty())
            .expect("failed Response object requires a named problem code");
        return Outcome::ProtocolFailures(BTreeSet::from([code.to_owned()]));
    }
    if response.status < 400 {
        Outcome::Success
    } else if let Some(code) = response.body["code"]
        .as_str()
        .or_else(|| response.body["error"]["code"].as_str())
    {
        Outcome::Problems(BTreeSet::from([code.to_owned()]))
    } else {
        Outcome::NonProblem
    }
}

fn assert_source(source: &str) {
    let (path, explanation) = source
        .split_once("# ")
        .expect("dispositions require a source path and explanation");
    assert!(
        explanation.trim().len() >= 12,
        "source disposition must explain reachability"
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    assert!(
        root.join(path.trim()).is_file(),
        "missing evidence source {path}"
    );
}

fn route_matches(operation: &str, method: &str, path: &str) -> bool {
    let Some((expected_method, template)) = operation.split_once(' ') else {
        return false;
    };
    let expected: Vec<_> = template.split('/').collect();
    let actual: Vec<_> = path.split('?').next().unwrap().split('/').collect();
    expected_method == method
        && expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(expected, actual)| {
            (*expected == actual)
                || (expected.starts_with('{') && expected.ends_with('}') && !actual.is_empty())
        })
}

#[test]
fn operation_templates_reject_wrong_routes_and_methods() {
    assert!(route_matches(
        "GET /api/v1/agents/{id}",
        "GET",
        "/api/v1/agents/$agent_id?limit=1"
    ));
    assert!(!route_matches(
        "GET /api/v1/agents/{id}",
        "POST",
        "/api/v1/agents/agt_contract"
    ));
    assert!(!route_matches(
        "GET /api/v1/agents/{id}",
        "GET",
        "/api/v1/workspaces/agt_contract"
    ));
}

#[test]
fn named_stream_failures_are_protocol_outcomes_not_http_successes() {
    use serde_json::json;

    for frames in [
        json!([{"event":"RUN_ERROR", "data":{"code":"agent_deleted"}}]),
        json!([{"data":{"type":"RUN_ERROR", "code":"agent_deleted"}}]),
        json!([{"event":"response.failed", "data":{"response":{"error":{"code":"agent_deleted"}}}}]),
        json!([{"event":"error", "data":{"error":{"code":"agent_deleted"}}}]),
        json!([{"event":"error", "data":{"code":"agent_deleted"}}]),
    ] {
        assert_eq!(
            outcome(&stream_response(frames)),
            Outcome::ProtocolFailures(BTreeSet::from(["agent_deleted".into()]))
        );
    }
    assert_eq!(
        outcome(&stream_response(json!([
            {"event":"RUN_STARTED", "data":{}},
            {"event":"RUN_ERROR", "data":{"code":"first_failure"}},
            {"event":"RUN_ERROR", "data":{"code":"second_failure"}}
        ]))),
        Outcome::ProtocolFailures(BTreeSet::from([
            "first_failure".into(),
            "second_failure".into()
        ]))
    );
}

#[test]
fn ordinary_streams_and_http_errors_keep_their_classification() {
    use serde_json::json;

    let mut response = stream_response(json!([
        {"event":"STATE_SNAPSHOT", "data":{"snapshot":{"error":{"code":"historical"}}}},
        {"event":"RUN_FINISHED", "data":{}}
    ]));
    assert_eq!(outcome(&response), Outcome::Success);
    response.body_kind = BodyKind::Json;
    response.status = 422;
    for body in [
        json!({"code":"invalid_agent_request"}),
        json!({"error":{"code":"invalid_agent_request"}}),
    ] {
        response.body = body;
        assert_eq!(
            outcome(&response),
            Outcome::Problems(BTreeSet::from(["invalid_agent_request".into()]))
        );
    }
    response.body = json!(null);
    assert_eq!(outcome(&response), Outcome::NonProblem);
}

#[test]
fn persisted_failed_response_objects_are_protocol_failures() {
    let mut response = stream_response(serde_json::json!({
        "object":"response", "status":"failed", "error":{"code":"agent_turn_failed"}
    }));
    response.body_kind = BodyKind::Json;
    assert_eq!(
        outcome(&response),
        Outcome::ProtocolFailures(BTreeSet::from(["agent_turn_failed".into()]))
    );
    response.body["status"] = serde_json::json!("completed");
    response.body["error"] = serde_json::Value::Null;
    assert_eq!(outcome(&response), Outcome::Success);
}

#[test]
#[should_panic(expected = "stream failure requires a named problem code")]
fn unnamed_stream_failures_cannot_pass_as_success() {
    outcome(&stream_response(serde_json::json!([
        {"event":"response.failed", "data":{"response":{"error":null}}}
    ])));
}

fn stream_response(body: serde_json::Value) -> ResponseFixture {
    ResponseFixture {
        status: 200,
        set_cookies: Vec::new(),
        headers: BTreeMap::new(),
        body,
        body_kind: BodyKind::Sse,
        stream_prefix: false,
    }
}
