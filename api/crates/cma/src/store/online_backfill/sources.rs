use super::super::agent_access_tools::AccessRecordAuthority;
use super::{Observation, Result, Snapshot};
use crate::config::AgentAccessMigration;
use layer2_core::sandbox::{Agent, AgentOperationIntent, AgentStatus, Sandbox, SandboxStatus};
use mongodb::bson::{doc, Document};

pub(super) enum Cohort {
    Routing {
        deployment: String,
        allocate_native: bool,
    },
    Endpoints {
        migration: AgentAccessMigration,
        authority: AccessRecordAuthority,
    },
}

const ROUTES: &str = "runtime_backend_routes";
const AGENTS: &str = "agents";
const SANDBOXES: &str = "sandboxes";
const ACCESS: &str = "agent_access_tools";

const PROVENANCE: &str = "agent_creation_provenance";

pub(super) const SOURCES: [&str; 5] = [ROUTES, AGENTS, SANDBOXES, ACCESS, PROVENANCE];

impl Cohort {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Routing { .. } => "runtime_routing",
            Self::Endpoints { .. } => "retained_agent_endpoints",
        }
    }

    pub(super) fn digest(&self) -> Result<String> {
        let result = match self {
            Self::Routing {
                deployment,
                allocate_native,
            } => {
                layer3_adapter::provider_request_digest(&(self.name(), deployment, allocate_native))
            }
            Self::Endpoints {
                migration,
                authority,
            } => layer3_adapter::provider_request_digest(&(
                self.name(),
                migration.revision,
                &migration.origin,
                &migration.agents,
                authority.namespace_scope(),
            )),
        };
        result.map_err(|_| "backfill cohort identity unavailable")
    }

    pub(super) fn source(&self) -> &'static str {
        match self {
            Self::Routing { .. } => SANDBOXES,
            Self::Endpoints { .. } => AGENTS,
        }
    }

    pub(super) fn selected_count(&self) -> i64 {
        match self {
            Self::Endpoints { migration, .. } if migration.agents.as_slice() != ["*"] => {
                i64::try_from(migration.agents.len()).expect("configured cohort is bounded")
            }
            _ => 0,
        }
    }

    pub(super) fn selection(&self) -> Document {
        if let Self::Endpoints { migration, .. } = self {
            if migration.agents.as_slice() != ["*"] {
                return doc! {"_id": {"$in": migration.agents.as_slice()}};
            }
        }
        doc! {}
    }

    pub(super) async fn observe(
        &self,
        snapshot: &Snapshot<'_>,
        row: &Document,
    ) -> Result<Observation> {
        match self {
            Self::Routing {
                deployment,
                allocate_native,
            } => {
                let Ok(sandbox) = super::super::sandboxes::read_sandbox(row) else {
                    return Ok(Observation::Error);
                };
                routing(snapshot, deployment, *allocate_native, &sandbox).await
            }
            Self::Endpoints {
                migration,
                authority,
            } => {
                let Ok(agent) = super::super::agents::read_agent(row) else {
                    return Ok(Observation::Error);
                };
                endpoint(snapshot, migration, authority, &agent).await
            }
        }
    }

    pub(super) async fn routing_initialized(
        &self,
        snapshot: &Snapshot<'_>,
    ) -> Result<Option<bool>> {
        match self {
            Self::Routing { deployment, .. } => {
                routing_initialized(snapshot, deployment).await.map(Some)
            }
            Self::Endpoints { .. } => Ok(None),
        }
    }
}

async fn routing_initialized(snapshot: &Snapshot<'_>, deployment: &str) -> Result<bool> {
    Ok(snapshot
        .one(
            ROUTES,
            doc! {
                "_id": format!("provider-routing-initialized:{deployment}"),
                "document_schema_version": 1,
                "deployment": deployment,
            },
        )
        .await?
        .is_some())
}

async fn routing(
    snapshot: &Snapshot<'_>,
    deployment: &str,
    allocate_native: bool,
    sandbox: &Sandbox,
) -> Result<Observation> {
    let generation = sandbox.lifecycle.runtime_generation_high_water.max(1);
    let id =
        layer3_adapter::provider_request_digest(&(deployment, sandbox.id.as_str(), generation))
            .map_err(|_| super::INVALID)?;
    let Some(route) = snapshot.one(ROUTES, doc! {"_id": id}).await? else {
        // The existing migration does not assign a provider to a retired
        // Sandbox or to an unbound Sandbox after native allocation is enabled.
        let skipped = sandbox.runtime.is_none()
            && allocate_native
            && (sandbox.status == SandboxStatus::Retired
                || routing_initialized(snapshot, deployment).await?);
        return Ok(if skipped {
            Observation::Skipped
        } else {
            Observation::Pending
        });
    };
    let known_backend = match route.get_str("backend").ok() {
        Some("agent-sandbox") => true,
        Some("opensandbox") => false,
        _ => return Ok(Observation::Error),
    };
    let valid = route.get_i64("document_schema_version").ok() == Some(1)
        && route.get_str("sandbox_id").ok() == Some(sandbox.id.as_str())
        && route.get_str("deployment").ok() == Some(deployment)
        && route
            .get_i64("generation")
            .ok()
            .and_then(|value| u64::try_from(value).ok())
            == Some(generation)
        && sandbox.runtime.as_ref().is_none_or(|binding| {
            known_backend == binding.runtime_id.starts_with("agent-sandbox:")
        });
    // Provider choices are immutable. The existing rows have no evidence of
    // whether startup backfill or ordinary allocation first recorded them.
    Ok(if valid {
        Observation::Retained
    } else {
        Observation::Error
    })
}

async fn endpoint(
    snapshot: &Snapshot<'_>,
    migration: &AgentAccessMigration,
    authority: &AccessRecordAuthority,
    agent: &Agent,
) -> Result<Observation> {
    if agent.status == AgentStatus::Deleted {
        return Ok(Observation::Skipped);
    }
    let Some(parent) = snapshot
        .one(SANDBOXES, doc! {"_id": agent.sandbox_id.as_str()})
        .await?
    else {
        return Ok(Observation::Error);
    };
    let Ok(sandbox) = super::super::sandboxes::read_sandbox(&parent) else {
        return Ok(Observation::Error);
    };
    if sandbox.owner != agent.owner {
        return Ok(Observation::Error);
    }
    if sandbox.status == SandboxStatus::Retired || !sandbox.credential.source.is_managed() {
        return Ok(Observation::Skipped);
    }
    let conversation = &agent.conversation;
    if let Some(operation) = &conversation.operation {
        if operation.failure.is_some()
            && matches!(
                &operation.intent,
                AgentOperationIntent::RefreshContext { revision, service_origin: Some(origin), .. }
                    if *revision == migration.revision && origin == &migration.origin
            )
        {
            return Ok(Observation::Error);
        }
    }
    let (Some(conversation_id), Some(runtime)) = (
        conversation.conversation_id.as_deref(),
        sandbox.retained_runtime_binding(),
    ) else {
        return Ok(Observation::Pending);
    };
    if conversation.runtime_generation.is_none()
        || runtime.provision_generation != conversation.runtime_generation
    {
        return Ok(Observation::Pending);
    }
    let Some(provenance) = snapshot
        .one(PROVENANCE, doc! {"_id": agent.id.as_str()})
        .await?
    else {
        return Ok(Observation::Pending);
    };
    let Ok(creation) = super::super::agent_provenance::read(&provenance) else {
        return Ok(Observation::Error);
    };
    if creation.validate_against(agent, &sandbox).is_err() {
        return Ok(Observation::Error);
    }

    let filter = authority
        .lookup_filter(&creation, runtime)
        .ok_or(super::INVALID)?;
    let records = snapshot.find(ACCESS, filter, 2, doc! {}).await?;
    let record = match records.as_slice() {
        [record] => record,
        [] => return Ok(Observation::Pending),
        _ => return Ok(Observation::Error),
    };
    let target = url::Url::parse(&migration.origin).map_err(|_| super::INVALID)?;
    match authority.confirmed_transport(
        record,
        &creation,
        runtime,
        conversation_id,
        target.as_str(),
    ) {
        Some(true) if conversation.managed_guidance_revision >= migration.revision => {
            Ok(Observation::Changed)
        }
        Some(_) => Ok(Observation::Pending),
        None => Ok(Observation::Error),
    }
}
