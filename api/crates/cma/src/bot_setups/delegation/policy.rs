use axum::http::Method;
use layer2_core::{bots::BotError, resource_access::ResourceOwner};

/// Match canonical local routes only. Neither headers nor proxy/auth/admin
/// destinations can be selected by the model. Existing handlers enforce access.
pub(super) fn validate(method: &Method, target: &str, key: Option<&str>) -> Result<(), BotError> {
    if target.len() > 4096 || target.chars().any(char::is_control) || target.contains(['#', '\\']) {
        return Err(refused());
    }
    let (path, _) = target.split_once('?').unwrap_or((target, ""));
    let parts = path.split('/').collect::<Vec<_>>();
    if !parts.iter().skip(1).all(|p| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }) {
        return Err(refused());
    }
    let read = method == Method::GET;
    let post = method == Method::POST;
    let update = method == Method::PUT;
    let delete = method == Method::DELETE;
    let allowed = match parts.as_slice() {
        ["", "api", "v1", "organizations"] => read,
        ["", "api", "v1", "sandboxes"] => read || post,
        ["", "api", "v1", "sandboxes", _] => read || delete,
        ["", "api", "v1", "sandboxes", _, "workspaces"] => read || post,
        ["", "api", "v1", "sandboxes", _, "commands" | "pause" | "revive"] => post,
        ["", "api", "v1", "sandboxes", _, "events"] => read,
        ["", "api", "v1", "sandbox-commands", _] => read,
        ["", "api", "v1", "workspaces", _] => read || delete,
        ["", "api", "v1", "workspaces", _, "agents"] => post,
        ["", "api", "v1", "workspaces", _, "worktrees"] => read,
        ["", "api", "v1", "worktrees", _] => read,
        ["", "api", "v1", "agents"] => read,
        ["", "api", "v1", "agents", _] => read || delete,
        ["", "api", "v1", "agent-profiles" | "workspace-profiles" | "sandbox-profiles" | "event-worker-profiles"] => {
            read || post
        }
        ["", "api", "v1", "agent-profiles" | "workspace-profiles" | "sandbox-profiles" | "event-worker-profiles", _] => {
            read || update
        }
        ["", "api", "v1", "agent-profiles" | "workspace-profiles" | "sandbox-profiles", _, "revisions"] => {
            read || post
        }
        ["", "api", "v1", "agent-profiles" | "workspace-profiles" | "sandbox-profiles" | "event-worker-profiles", _, "revisions", _] => {
            read
        }
        ["", "api", "v1", "agent-profiles" | "workspace-profiles" | "sandbox-profiles", _, "retire"] => {
            post
        }
        ["", "api", "v1", "profile-authoring", "bounds" | "services" | "skills"] => read,
        ["", "api", "v1", "marketplace", _] | ["", "api", "v1", "marketplace", _, _] => read,
        ["", "api", "v1", "marketplace", _, _, "fork"] => post,
        ["", "api", "v2", "responses"] => post,
        ["", "api", "v2", "responses" | "commands" | "conversations", _] => read,
        ["", "api", "v2", "conversations", _, "items"] => read,
        ["", "api", "v2", "agents", _, "responses"] => post,
        ["", "api", "v2", "agents", _, "responses", _, "controls" | "queued-cancellation"] => {
            read || post
        }
        _ => false,
    };
    if !allowed
        || (!read
            && key.is_none_or(|v| {
                v.is_empty() || v.len() > 256 || !v.bytes().all(|b| b.is_ascii_graphic())
            }))
    {
        return Err(refused());
    }
    Ok(())
}
fn refused() -> BotError {
    BotError::Invalid("This operation is not available through delegated CMA access; mutations require an idempotency key.".into())
}

/// These legacy creation handlers infer personal ownership from the caller.
/// Organization membership must never silently create a personal resource.
pub(super) fn validate_owner(
    method: &Method,
    target: &str,
    owner: &ResourceOwner,
) -> Result<(), BotError> {
    if method != Method::POST || !matches!(owner, ResourceOwner::Organization { .. }) {
        return Ok(());
    }
    let path = target.split('?').next().unwrap_or_default();
    if matches!(
        path.split('/').collect::<Vec<_>>().as_slice(),
        [
            "",
            "api",
            "v1",
            "sandboxes"
                | "agent-profiles"
                | "workspace-profiles"
                | "sandbox-profiles"
                | "event-worker-profiles"
        ] | ["", "api", "v1", "marketplace", _, _, "fork"]
    ) {
        return Err(BotError::Invalid(
            "This creation operation supports personal ownership only. Organization Bot creation requires the Bot setup action.".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_operations_are_bounded_and_require_mutation_identity() {
        for path in [
            "/api/v1/sandboxes?organization=org-a",
            "/api/v1/agents/agt_one",
            "/api/v2/responses/resp_one",
        ] {
            assert!(validate(&Method::GET, path, None).is_ok());
        }
        assert!(validate(
            &Method::POST,
            "/api/v1/workspaces/wks_one/agents",
            Some("create-one")
        )
        .is_ok());
        assert!(validate(&Method::POST, "/api/v1/workspaces/wks_one/agents", None).is_err());
    }
    #[test]
    fn authority_routes_and_encoded_or_external_destinations_are_never_forwarded() {
        for path in [
            "https://other.test/api/v1/sandboxes",
            "//other.test/api/v1/sandboxes",
            "/api/v1/admin/settings",
            "/api/v1/cmaeg/bindings",
            "/api/v1/agent-delegations/operations",
            "/auth/logout",
            "/api/v1/sandboxes/%2e%2e/admin",
            "/api/v1/sandboxes/../admin",
            "/api/v1/sandboxes//x",
            "/api/v1/sandboxes/x#y",
            "/api/v1/bots",
        ] {
            assert!(validate(&Method::GET, path, None).is_err(), "{path}");
            assert!(
                validate(&Method::POST, path, Some("key")).is_err(),
                "{path}"
            );
        }
    }
}
