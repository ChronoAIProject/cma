//! Scope metadata restricts eligibility; only live provider evidence grants it.
use super::{bearer, AuthState};
use base64::Engine as _;
use serde_json::Value;

impl AuthState {
    pub fn with_oauth_api_resource(
        mut self: std::sync::Arc<Self>,
        resource: Option<String>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::get_mut(&mut self)
            .expect("OAuth API resource is configured before sharing auth")
            .oauth_api_resource = resource;
        self
    }
    pub(super) fn permits_oauth_or_key_commands(&self, token: &str) -> bool {
        permits(token, self.oauth_api_resource.as_deref())
    }
}
fn empty_array(body: &Value, field: &str) -> bool {
    body.get(field)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
}
fn resources_allow(value: &Value, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items.len() <= 128
            && items
                .iter()
                .all(|v| v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 4096))
            && items.iter().any(|v| v.as_str() == Some(expected))
    })
}
/// This is only a negative filter. The provider must verify the exact bytes,
/// current consent, RBAC, expiry and the positive resource match separately.
pub(super) fn permits(token: &str, expected: Option<&str>) -> bool {
    if bearer::permits_independent_commands(token) {
        return true;
    }
    if !bearer::permits_personal_authority(token) {
        return false;
    }
    let Some(body) = claims(token) else {
        return false;
    };
    if body.get("token_type").and_then(Value::as_str) != Some("access")
        || ["cnf", "api_key_id", "assistant_forward", "resource"]
            .iter()
            .any(|k| body.get(*k).is_some_and(|v| !v.is_null()))
        || body.get("allow_all_nodes").and_then(Value::as_bool) != Some(true)
        || !empty_array(&body, "allowed_node_ids")
    {
        return false;
    }
    match body.get("allow_all_services").and_then(Value::as_bool) {
        Some(true) => empty_array(&body, "resources") && empty_array(&body, "allowed_service_ids"),
        Some(false) => {
            body.get("resources")
                .is_some_and(|v| resources_allow(v, expected))
                && body
                    .get("scope")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.split_whitespace().any(|s| s == "proxy"))
                && body
                    .get("allowed_service_ids")
                    .and_then(Value::as_array)
                    .is_some_and(|ids| {
                        !ids.is_empty()
                            && ids.len() <= 128
                            && ids.iter().all(|id| {
                                id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 256)
                            })
                    })
        }
        None => false,
    }
}
fn claims(token: &str) -> Option<Value> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}
pub(super) fn requires_resource(token: &str) -> bool {
    claims(token)
        .is_some_and(|b| b.get("allow_all_services").and_then(Value::as_bool) == Some(false))
}
/// An authenticated provider observation for this exact bearer. A restricted
/// token needs a positive resource match, never an empty consent list.
pub(super) fn provider_allows(body: &Value, expected: Option<&str>, required: bool) -> bool {
    if required {
        return body
            .get("resource")
            .is_some_and(|v| resources_allow(v, expected))
            && body
                .get("scope")
                .and_then(Value::as_str)
                .is_some_and(|s| s.split_whitespace().any(|s| s == "proxy"));
    }
    match body.get("resource") {
        None | Some(Value::Null) => true,
        Some(Value::Array(items)) if items.is_empty() => true,
        Some(value) => {
            resources_allow(value, expected)
                && body
                    .get("scope")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.split_whitespace().any(|s| s == "proxy"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const RESOURCE: &str = "https://issuer.example/api/v1/proxy/s/cma";
    fn token(body: &Value) -> String {
        format!(
            "e30.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(body).unwrap())
        )
    }
    fn scoped() -> Value {
        json!({"token_type":"access","scope":"openid proxy","resources":[RESOURCE],"allowed_service_ids":["connection"],"allow_all_services":false,"allowed_node_ids":[],"allow_all_nodes":true})
    }
    #[test]
    fn nyxid_user_scope_shapes_require_explicit_command_authority() {
        let mut body = scoped();
        assert!(permits(&token(&body), Some(RESOURCE)));
        assert!(!permits(&token(&body), None));
        assert!(!permits(
            &token(&body),
            Some("https://other.example/api/v1/proxy/s/cma")
        ));
        body["resources"] = json!([]);
        body["allowed_service_ids"] = json!([]);
        assert!(!permits(&token(&body), Some(RESOURCE)));
        body["allow_all_services"] = json!(true);
        assert!(permits(&token(&body), None));
    }
    #[test]
    fn restricted_or_non_user_credentials_cannot_acquire_ambient_authority() {
        for (key, value) in [
            ("scope", json!("openid")),
            ("resources", json!([])),
            ("resources", json!(["other"])),
            ("allowed_service_ids", json!([])),
            ("allowed_node_ids", json!(["node"])),
            ("allow_all_nodes", json!(false)),
            ("act", json!({"sub":"delegate"})),
            ("sa", json!(true)),
            ("delegated", json!(true)),
            ("relay", json!(true)),
            ("cnf", json!({"jkt":"key"})),
            ("api_key_id", json!("key")),
            ("assistant_forward", json!(true)),
            ("token_type", json!("id_token")),
        ] {
            let mut body = scoped();
            body[key] = value;
            assert!(!permits(&token(&body), Some(RESOURCE)), "accepted {key}");
        }
    }
    #[test]
    fn provider_evidence_requires_the_configured_resource_and_service_scope() {
        assert!(provider_allows(
            &json!({"resource":[RESOURCE],"scope":"proxy"}),
            Some(RESOURCE),
            true
        ));
        for body in [
            json!({"resource":[RESOURCE]}),
            json!({"resource":["other"],"scope":"proxy"}),
            json!({"resource":"invalid","scope":"proxy"}),
        ] {
            assert!(!provider_allows(&body, Some(RESOURCE), true));
        }
        assert!(!provider_allows(
            &json!({"resource":[RESOURCE],"scope":"proxy"}),
            None,
            true
        ));
    }
}
