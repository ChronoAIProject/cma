use super::context::Case;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classification {
    Local,
    TypedShape,
    MediaType,
    RawJson,
    BodySize,
    QueryShape,
    BinaryHeaders,
}

// Exact backend-invalid cases not claimed as typed HTTP replay. Each is checked
// against its source shape or the SDK's real preflight, then its shared error is decoded.
#[path = "classification/agent_inputs.rs"]
mod agent_inputs;
#[path = "classification/conversation_inputs.rs"]
mod conversation_inputs;
#[path = "classification/hierarchy_inputs.rs"]
mod hierarchy_inputs;
#[path = "classification/operator_and_wire_inputs.rs"]
mod operator_and_wire_inputs;

pub fn local_cases() -> impl Iterator<Item = &'static (&'static str, Classification)> {
    [
        agent_inputs::CASES,
        hierarchy_inputs::CASES,
        conversation_inputs::CASES,
        operator_and_wire_inputs::CASES,
    ]
    .into_iter()
    .flatten()
}

pub fn classify(case: &Case) -> Option<Classification> {
    local_cases()
        .find(|(id, _)| *id == case.id)
        .map(|(_, kind)| *kind)
}

pub fn assert_unrepresentable(case: &Case, class: Classification) {
    match class {
        Classification::BinaryHeaders => {
            let headers = &case.request.headers;
            let invalid = match case.id.as_str() {
                "binding-media-missing-length" => !headers.contains_key("content-length"),
                "binding-media-missing-type" => !headers.contains_key("content-type"),
                "binding-media-encoding" => {
                    headers.get("content-encoding").map(String::as_str) == Some("gzip")
                }
                "binding-media-too-large" => {
                    headers["content-length"].parse::<usize>().unwrap() > 20 * 1024 * 1024
                }
                "binding-media-incomplete-body" => {
                    headers["content-length"].parse::<usize>().unwrap()
                        != case.request.expanded_raw_body().unwrap().len()
                }
                _ => false,
            };
            assert!(
                invalid,
                "{} typed binary upload derives exact headers",
                case.id
            );
        }
        Classification::MediaType => {
            let content_type = case.request.headers.get("content-type").map(String::as_str);
            assert_eq!(
                content_type,
                (case.id == "authoring-resolve-wrong-content-type").then_some("text/plain"),
                "{}",
                case.id
            );
        }
        Classification::RawJson => assert!(
            serde_json::from_str::<serde_json::Value>(&case.request.expanded_raw_body().unwrap())
                .is_err(),
            "{}",
            case.id
        ),
        Classification::BodySize => {
            let raw = case.request.expanded_raw_body().unwrap();
            assert!(raw.len() > 2 * 1024 * 1024, "{}", case.id);
            assert!(raw.bytes().all(|byte| byte == b' '), "{}", case.id);
            assert_eq!(case.response.status, 413, "{}", case.id);
        }
        Classification::QueryShape => {
            let url = reqwest::Url::parse(&format!("http://fixture{}", case.request.path)).unwrap();
            if case.id.starts_with("agent-events-v2-") && case.id.ends_with("-duplicate") {
                let key = if case.id.contains("snapshot") {
                    "page_cursor"
                } else {
                    "after_cursor"
                };
                assert_eq!(url.query_pairs().filter(|(name, _)| name == key).count(), 2);
                return;
            }
            if case.id == "authoring-skills-duplicate-query" {
                assert_eq!(url.query_pairs().filter(|(key, _)| key == "q").count(), 2);
                return;
            }
            let allowed: &[&str] = match case.operation.as_str() {
                "GET /api/v1/desktop-capability" => &["workspace_id", "sandbox_id"],
                "GET /api/v1/agents/{id}/files/tree"
                | "GET /api/v1/agents/{id}/files/content"
                | "GET /api/v1/agents/{id}/files/download"
                | "DELETE /api/v1/agents/{id}/files" => &["path", "max_bytes", "recursive"],
                "GET /api/v2/agents/{agent_id}/events/snapshot" => &["limit", "page_cursor"],
                "GET /api/v2/agents/{agent_id}/events/head" => &[],
                "GET /api/v2/agents/{agent_id}/events" => &["after_cursor", "follow"],
                "GET /api/v2/agents/{agent_id}/queued-inputs" => &["after", "limit"],
                "GET /api/v1/bridge-connectors/capabilities"
                | "GET /api/v1/bridge-creation-target" => &["organization"],
                "GET /api/v1/bridges" => &["organization", "cursor", "limit"],
                "GET /api/v1/agents/provenance" => &[
                    "organization",
                    "sandbox",
                    "workspace",
                    "status",
                    "creation_origin",
                    "cursor",
                    "limit",
                ],
                "GET /api/v1/sandboxes" => &["organization", "status", "cursor", "limit"],
                "GET /api/v1/agents" => &[
                    "organization",
                    "sandbox",
                    "workspace",
                    "status",
                    "cursor",
                    "limit",
                ],
                "GET /api/v1/profile-authoring/skills" => &["q", "limit", "cursor"],
                "GET /api/v1/agui/events" => &["thread_id", "starting_after", "follow"],
                "GET /api/v1/conversations/{id}/items" => &["after", "limit"],
                "GET /api/v1/triggers/{id}/embed" => &["placement_id"],
                "GET /api/v1/agui/launches/{id}/events" => &["starting_after", "limit", "follow"],
                "GET /api/v1/responses/{response_id}"
                | "GET /api/v2/responses/{response_id}"
                | "GET /api/v2/agents/{agent_id}/responses/{response_id}/controls"
                | "GET /api/v2/commands/{command_id}"
                | "GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation"
                | "POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation"
                | "POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input"
                | "POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt" => &[],
                _ => panic!("unclassified query shape {}", case.id),
            };
            assert!(
                url.query_pairs()
                    .any(|(key, _)| !allowed.contains(&key.as_ref())),
                "{}",
                case.id
            );
        }
        Classification::Local | Classification::TypedShape => {
            panic!("input cases must exercise typed SDK construction or preflight")
        }
    }
}
