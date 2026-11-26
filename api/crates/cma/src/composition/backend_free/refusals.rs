//! Keep the normal route and method boundary when execution is disabled.
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use axum::{
    extract::{rejection::PathRejection, MatchedPath, Path, Query, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{MethodFilter, MethodRouter},
    Extension, Router,
};
use layer1_public::{
    auth::NotAuthenticated,
    responses::{
        parse_response_id, response_not_found,
        v2::{authentication_required, resource_not_found},
    },
    rest::{
        worker_credentials::{VerifiedGatewayService, WORKER_CLASSIFIER_PATH},
        HttpProblem, Problem,
    },
    AuthenticatedCaller, AuthenticatedResourceCaller,
};
use layer2_core::{
    resource_access::{
        CredentialSource, ResourceAccessStoreError, ResourceLineage, ResourceTarget,
    },
    AgentId, Capability, CoreError, LaunchId, SandboxId, WorkspaceId, WorktreeId,
};
use mongodb::bson::doc;

use crate::store::mongo::MongoAccountStores;

const LEGACY_RESPONSE_PATH: &str = "/api/v1/responses/{response_id}";

struct RefusalState {
    stores: Arc<MongoAccountStores>,
    desktop_origins: Vec<String>,
}

pub(super) fn router(stores: Arc<MongoAccountStores>, desktop_origins: Vec<String>) -> Router {
    let mut router = Router::new();
    for (path, operations) in layer1_public::rest::openapi::document().paths.paths {
        if matches!(
            path.as_str(),
            "/api/v1/admin/settings"
                | "/api/v1/admin/settings/reset"
                | "/api/v1/conformance"
                | "/auth/signup-options"
        ) {
            continue;
        }
        let mut methods = MethodRouter::new();
        let mut has_refusals = false;
        for (operation, method) in [
            (
                operations.get.filter(|_| path != "/api/v1/agents"),
                MethodFilter::GET,
            ),
            (operations.post, MethodFilter::POST),
            (operations.put, MethodFilter::PUT),
            (operations.patch, MethodFilter::PATCH),
            (operations.delete, MethodFilter::DELETE),
        ] {
            if let Some(operation) = operation {
                has_refusals = true;
                if path == WORKER_CLASSIFIER_PATH {
                    methods = methods.on(method, worker_unavailable);
                } else if operation
                    .security
                    .is_some_and(|security| security.is_empty())
                {
                    methods = methods.on(method, public_unavailable);
                } else if path == LEGACY_RESPONSE_PATH && method == MethodFilter::GET
                    || path == "/api/v2/commands/{command_id}"
                    || path.starts_with("/api/v2/") && path.contains("/{response_id}")
                {
                    methods = methods.on(method, command_unavailable);
                } else {
                    methods = methods.on(method, unavailable);
                }
            }
        }
        if has_refusals {
            router = router.route(&path, methods);
        }
    }
    for path in [
        "/auth/connections/check",
        "/auth/connections/repository-access",
        "/auth/connections/start",
        "/auth/connections/cancel",
    ] {
        router = router.route(path, MethodRouter::new().post(unavailable));
    }
    router.with_state(Arc::new(RefusalState {
        stores,
        desktop_origins,
    }))
}

async fn public_unavailable() -> HttpProblem {
    HttpProblem::SandboxBackendUnavailable
}

async fn worker_unavailable(Extension(_service): Extension<VerifiedGatewayService>) -> HttpProblem {
    HttpProblem::SandboxBackendUnavailable
}

async fn command_unavailable(
    State(state): State<Arc<RefusalState>>,
    auth: Result<AuthenticatedResourceCaller, NotAuthenticated>,
    matched: MatchedPath,
    Path(parameters): Path<BTreeMap<String, String>>,
) -> Response {
    let legacy = matched.as_str() == LEGACY_RESPONSE_PATH;
    let auth = match auth {
        Ok(auth) => auth,
        Err(error) if legacy => return error.into_response(),
        Err(_) => return authentication_required(),
    };
    let missing = if legacy {
        response_not_found
    } else {
        resource_not_found
    };
    let context = &auth.context;
    if let Err(error) = context.caller.require(Capability::UseOwnSandboxes) {
        return Problem(error).into_response();
    }
    if context.credential_source == CredentialSource::Binding {
        return missing();
    }
    let authorization = async {
        if legacy {
            let (agent_id, turn) = parameters
                .get("response_id")
                .and_then(|id| parse_response_id(id))
                .ok_or(ResourceAccessStoreError::NotFound)?;
            return state
                .stores
                .authorize_personal_turn(&context.caller, agent_id, turn)
                .await;
        }
        let selector = if let Some(response) = parameters.get("response_id") {
            doc! { "response_id": response }
        } else if let Some(command) = parameters.get("command_id") {
            doc! { "_id": command }
        } else {
            return Err(ResourceAccessStoreError::NotFound);
        };
        state
            .stores
            .authorize_personal_command(
                &context.caller,
                selector,
                parameters.get("agent_id").map(String::as_str),
            )
            .await
    };
    match tokio::time::timeout(Duration::from_secs(15), authorization).await {
        Ok(Ok(())) if context.validate_principal() => {
            HttpProblem::SandboxBackendUnavailable.into_response()
        }
        Ok(Ok(()) | Err(ResourceAccessStoreError::NotFound)) => missing(),
        Ok(Err(error)) => {
            tracing::debug!(%error, "backend-free command authorization failed");
            missing()
        }
        Err(_) => {
            tracing::debug!("backend-free command authorization timed out");
            missing()
        }
    }
}

#[derive(serde::Deserialize)]
struct AguiThread {
    thread_id: String,
}

async fn unavailable(
    State(state): State<Arc<RefusalState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    method: Method,
    matched: MatchedPath,
    headers: HeaderMap,
    parameters: Result<Path<BTreeMap<String, String>>, PathRejection>,
    thread: Result<Query<AguiThread>, axum::extract::rejection::QueryRejection>,
) -> Result<HttpProblem, Response> {
    let path = matched.as_str();
    if path.ends_with("/desktop/ws")
        && !layer1_public::rest::desktop_origin_permitted(&headers, &state.desktop_origins)
    {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    if path == "/api/v1/triggers/public/{public_id}/launches" {
        if !caller.authorization.allows(Capability::LaunchTriggers)
            && !caller.authorization.allows(Capability::CreateSandbox)
        {
            return Err(Problem(CoreError::Forbidden).into_response());
        }
    } else if path.starts_with("/api/v1/launches/{id}")
        || path == "/api/v1/agui/launches/{id}/events"
    {
        let id = parameter(&parameters, "id").map_err(IntoResponse::into_response)?;
        if !state
            .stores
            .owns_launch(&caller.subject, &LaunchId(id.to_owned()))
            .await
            .map_err(|error| Problem(error).into_response())?
        {
            return Err(HttpProblem::NotFound.into_response());
        }
    } else {
        caller
            .require(required_capability(&method, path))
            .map_err(|error| Problem(error).into_response())?;
    }
    if !path.starts_with("/api/v1/admin/") {
        let targets = if path == "/api/v1/agui/events" {
            let Query(AguiThread { thread_id }) =
                thread.map_err(|_| invalid_resource_selector().into_response())?;
            vec![ResourceTarget::Agent {
                agent_id: AgentId(thread_id),
            }]
        } else {
            targets(path, parameters).map_err(IntoResponse::into_response)?
        };
        if let Some(target) = targets.last() {
            let snapshot = state
                .stores
                .personal_resource(&caller, target)
                .await
                .map_err(|error| match error {
                    ResourceAccessStoreError::NotFound => HttpProblem::NotFound.into_response(),
                    _ => Problem(CoreError::RuntimeUnavailable(
                        "resource authorization is unavailable".into(),
                    ))
                    .into_response(),
                })?;
            if !targets
                .iter()
                .all(|target| contains(&snapshot.lineage, target))
            {
                return Err(HttpProblem::NotFound.into_response());
            }
        }
    }
    Ok(HttpProblem::SandboxBackendUnavailable)
}

fn required_capability(method: &Method, path: &str) -> Capability {
    match (method, path) {
        (&Method::POST, "/api/v1/sandboxes" | "/api/v1/sandboxes/desktop") => {
            Capability::CreateSandbox
        }
        (&Method::DELETE, "/api/v1/sandboxes/{id}") => Capability::DeleteOwnSandbox,
        (_, "/api/v1/admin/runtime-inventory") => Capability::ReadRuntimeInventory,
        (_, "/api/v1/admin/activity") => Capability::ReadActivityLog,
        (_, "/api/v1/admin/users") => Capability::ReadUserDirectory,
        (&Method::GET | &Method::HEAD, path) if path.starts_with("/api/v1/admin/sandboxes") => {
            Capability::ReadAllSandboxes
        }
        (_, path) if path.starts_with("/api/v1/admin/sandboxes") => Capability::ControlAnySandbox,
        (_, path) if path.starts_with("/api/v1/admin/") => Capability::ConfigurePlatform,
        (_, path) if path.starts_with("/api/v1/bots") => Capability::ManageOwnBots,
        (_, path) if path.starts_with("/api/v1/cmaeg/") => Capability::ManageProviderBindings,
        (_, path) if path.starts_with("/api/v1/gateway/") => Capability::ManageGatewayRecords,
        (&Method::GET | &Method::POST, "/api/v1/triggers") => Capability::PublishTriggers,
        (_, path)
            if path.starts_with("/api/v1/agent-profiles")
                || path.starts_with("/api/v1/workspace-profiles")
                || path.starts_with("/api/v1/sandbox-profiles") =>
        {
            Capability::ManageOwnProfiles
        }
        _ => Capability::UseOwnSandboxes,
    }
}

fn invalid_resource_selector() -> Problem {
    Problem(CoreError::InvalidRequest(
        "invalid resource selector".into(),
    ))
}

fn parameter<'a>(
    parameters: &'a Result<Path<BTreeMap<String, String>>, PathRejection>,
    name: &str,
) -> Result<&'a str, Problem> {
    parameters
        .as_ref()
        .ok()
        .and_then(|params| params.get(name))
        .map(String::as_str)
        .ok_or_else(invalid_resource_selector)
}

fn targets(
    path: &str,
    parameters: Result<Path<BTreeMap<String, String>>, PathRejection>,
) -> Result<Vec<ResourceTarget>, Problem> {
    let parts = path.split('/').collect::<Vec<_>>();
    let mut targets = Vec::new();
    for parts in parts.windows(2) {
        let Some(name) = parts[1]
            .strip_prefix('{')
            .and_then(|part| part.strip_suffix('}'))
        else {
            continue;
        };
        if !(matches!(
            parts[0],
            "sandboxes" | "workspaces" | "worktrees" | "agents"
        ) || parts[0] == "conversations"
            && (path.starts_with("/api/v1/conversations/")
                || path.starts_with("/api/v2/conversations/")))
        {
            continue;
        }
        let value = parameter(&parameters, name)?.to_owned();
        targets.push(match parts[0] {
            "sandboxes" => ResourceTarget::Sandbox {
                sandbox_id: SandboxId(value),
            },
            "workspaces" => ResourceTarget::Workspace {
                workspace_id: WorkspaceId(value),
            },
            "worktrees" => ResourceTarget::Worktree {
                worktree_id: WorktreeId(value),
            },
            "agents" | "conversations" => ResourceTarget::Agent {
                agent_id: AgentId(value),
            },
            _ => unreachable!("known resource collection"),
        });
    }
    Ok(targets)
}

fn contains(lineage: &ResourceLineage, target: &ResourceTarget) -> bool {
    match target {
        ResourceTarget::Sandbox { sandbox_id } => lineage.sandbox.id == *sandbox_id,
        ResourceTarget::Workspace { workspace_id } => lineage
            .workspace
            .as_ref()
            .is_some_and(|row| row.id == *workspace_id),
        ResourceTarget::Worktree { worktree_id } => lineage
            .worktree
            .as_ref()
            .is_some_and(|row| row.id == *worktree_id),
        ResourceTarget::Agent { agent_id } => lineage
            .agent
            .as_ref()
            .is_some_and(|row| row.id == *agent_id),
    }
}
