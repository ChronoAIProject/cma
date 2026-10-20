use super::context::{Context, invalid};
use cma_sdk::{
    Result,
    agents::{AccessibleAgentListOptions, AgentListOptions, AgentProvenanceInventoryOptions},
    instances::{AccessibleSandboxListOptions, SandboxListOptions},
    profiles::{
        AgentProfileContent, MarketplaceListOptions, ProfileContent, Profiles,
        SandboxProfileContent, WorkspaceProfileContent,
    },
};
use std::fmt::Debug;

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let client = c.client;
    let op = c.case.operation.as_str();
    if op.contains("/agent-profiles") {
        return profiles(c, client.agent_profiles()).await;
    }
    if op.contains("/workspace-profiles") {
        return profiles(c, client.workspace_profiles()).await;
    }
    if op.contains("/sandbox-profiles") {
        return profiles(c, client.sandbox_profiles()).await;
    }
    if op.contains("/marketplace/") {
        return match c.param("kind") {
            "agent-profiles" => marketplace::<AgentProfileContent>(c).await,
            "workspace-profiles" => marketplace::<WorkspaceProfileContent>(c).await,
            "sandbox-profiles" => marketplace::<SandboxProfileContent>(c).await,
            _ => Err(invalid("unsupported marketplace kind")),
        };
    }
    match op {
        "GET /api/v1/agents/{id}/models" => c.check(client.agents().models(c.param("id")).await),
        "GET /api/v1/agents/{id}/skills" => c.check(client.agents().skills(c.param("id")).await),
        "POST /api/v1/agents/{id}/skills" => c.check(
            client
                .agents()
                .install_skill(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "PUT /api/v1/agents/{id}/service-access" => c.check(
            client
                .agents()
                .update_service_access(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/agents/{id}/access-requests" => {
            c.check(client.agents().access_requests(c.param("id")).await)
        }
        "POST /api/v1/agents/{id}/access-requests/{request_id}/approve" => c.check(
            client
                .agents()
                .approve_access_request(c.param("id"), c.param("request_id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/agents/{id}/access-requests/{request_id}/deny" => c.check(
            client
                .agents()
                .deny_access_request(c.param("id"), c.param("request_id"), &c.key()?)
                .await,
        ),
        "GET /api/v1/agents/{id}/message-feedback" => c.check(
            client
                .agents()
                .message_feedback(c.param("id"), c.events()?)
                .await,
        ),
        "PUT /api/v1/agents/{id}/messages/{message_id}/feedback" => {
            #[derive(serde::Deserialize)]
            struct FeedbackBody {
                vote: Option<cma_sdk::agents::MessageVote>,
            }
            let body: FeedbackBody = c.body()?;
            c.check(
                client
                    .agents()
                    .set_message_feedback(c.param("id"), c.param("message_id"), body.vote)
                    .await,
            )
        }
        "GET /api/v1/agents/{id}/settings" => {
            c.check(client.agents().settings(c.param("id")).await)
        }
        "PATCH /api/v1/agents/{id}/settings" => c.check(
            client
                .agents()
                .update_settings(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/agents" => c.check(
            client
                .agents()
                .create_lifecycle(&c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/agent-lifecycle/{id}" => {
            c.check(client.agents().read_lifecycle(c.param("id")).await)
        }
        "POST /api/v1/sandboxes/{id}/workspaces/inline" => c.check(
            client
                .workspaces()
                .create_inline(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "PUT /api/v1/gateway/channel-records/{record_id}" => c.check(
            client
                .gateway_records()
                .put(c.param("record_id"), &c.body()?)
                .await,
        ),
        "GET /api/v1/gateway/channel-records/{record_id}" => {
            c.check(client.gateway_records().get(c.param("record_id")).await)
        }
        "GET /api/v1/gateway/channel-records" => c.check(
            client
                .gateway_records()
                .list(&cma_sdk::gateway_records::GatewayRecordListOptions {
                    owner: c.q("owner"),
                    after: c.q("after"),
                    limit: c.number("limit")?,
                })
                .await,
        ),
        "DELETE /api/v1/gateway/channel-records/{record_id}" => c.check(
            client
                .gateway_records()
                .delete(c.param("record_id"), c.number("expected_version")?)
                .await,
        ),
        "PUT /api/v1/gateway/connection-records/{record_id}" => c.check(
            client
                .gateway_connection_records()
                .put(c.param("record_id"), &c.body()?)
                .await,
        ),
        "GET /api/v1/gateway/connection-records/{record_id}" => c.check(
            client
                .gateway_connection_records()
                .get(c.param("record_id"))
                .await,
        ),
        "GET /api/v1/gateway/connection-records" => c.check(
            client
                .gateway_connection_records()
                .list(&cma_sdk::gateway_records::GatewayRecordListOptions {
                    owner: c.q("owner"),
                    after: c.q("after"),
                    limit: c.number("limit")?,
                })
                .await,
        ),
        "DELETE /api/v1/gateway/connection-records/{record_id}" => c.check(
            client
                .gateway_connection_records()
                .delete(c.param("record_id"), c.number("expected_version")?)
                .await,
        ),
        "GET /api/v1/agents/{id}/channel-usage" => {
            c.check(client.agents().channel_usage(c.param("id")).await)
        }
        "GET /api/v1/agents/{id}/channel-usage/history" => c.check(
            client
                .agents()
                .channel_usage_history(c.param("id"), &c.page()?)
                .await,
        ),
        "GET /api/v1/agents/{id}/artifacts" => c.check(
            client
                .agents()
                .list_artifacts(c.param("id"), &c.page()?)
                .await,
        ),
        "GET /api/v1/agents/{id}/artifacts/{artifact_id}" => c.check(
            client
                .agents()
                .read_artifact(c.param("id"), c.param("artifact_id"))
                .await,
        ),
        "POST /api/v1/agents/{id}/attachments" => c.check(
            client
                .agents()
                .upload_attachment(
                    c.param("id"),
                    c.query.get("name").map(String::as_str).unwrap_or(""),
                    c.case
                        .request
                        .headers
                        .get("content-type")
                        .map(String::as_str)
                        .unwrap_or("application/octet-stream"),
                    c.case
                        .request
                        .expanded_raw_body()
                        .unwrap_or_default()
                        .into_bytes(),
                    &c.key()?,
                )
                .await,
        ),
        "POST /api/v1/agents/{id}/artifacts" => c.check(
            client
                .agents()
                .publish_artifact(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/agents/{id}/artifacts/{artifact_id}/download" => c.check(
            client
                .agents()
                .download_artifact(c.param("id"), c.param("artifact_id"))
                .await
                .map(|bytes| String::from_utf8(bytes).unwrap()),
        ),
        "POST /api/v1/sandboxes/{id}/runtime-update" => c.check(
            client
                .sandboxes()
                .update_runtime(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/sandboxes/{id}/commands" => c.check(
            client
                .sandboxes()
                .submit_command(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/sandbox-commands/{id}" => {
            c.check(client.sandboxes().read_command(c.param("id")).await)
        }
        "GET /api/v1/organizations" => c.check(client.resource_access().organizations().await),
        "GET /api/v1/resource-access/{kind}/{id}" => c.check(
            client
                .resource_access()
                .effective(c.param("kind"), c.param("id"))
                .await,
        ),
        "GET /api/v1/resource-shares/{kind}/{id}" => c.check(
            client
                .resource_access()
                .list_shares(c.param("kind"), c.param("id"))
                .await,
        ),
        "GET /api/v1/resource-shares/{kind}/{id}/{share_id}" => c.check(
            client
                .resource_access()
                .read_share(c.param("kind"), c.param("id"), c.param("share_id"))
                .await,
        ),
        "POST /api/v1/resource-shares/{kind}/{id}" => c.check(
            client
                .resource_access()
                .create_share(c.param("kind"), c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "PATCH /api/v1/resource-shares/{kind}/{id}/{share_id}" => c.check(
            client
                .resource_access()
                .update_share(
                    c.param("kind"),
                    c.param("id"),
                    c.param("share_id"),
                    &c.body()?,
                    &c.key()?,
                )
                .await,
        ),
        "DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}" => c.check(
            client
                .resource_access()
                .revoke_share(
                    c.param("kind"),
                    c.param("id"),
                    c.param("share_id"),
                    c.number("expected_revision")?
                        .ok_or_else(|| invalid("revision required"))?,
                    &c.key()?,
                )
                .await,
        ),
        "GET /api/v1/sandboxes" => c.check(
            client
                .sandboxes()
                .list_accessible(&AccessibleSandboxListOptions {
                    organization: c.q("organization"),
                    shared: c.number::<bool>("shared")?.unwrap_or(false),
                    filters: SandboxListOptions {
                        status: c.tag("status")?,
                        page: c.page()?,
                    },
                })
                .await,
        ),
        "GET /api/v1/sandboxes/{id}" => {
            c.check(client.sandboxes().read_accessible(c.param("id")).await)
        }
        "GET /api/v1/sandboxes/{id}/events" => {
            c.check(client.sandboxes().events(c.param("id"), c.events()?).await)
        }
        "POST /api/v1/sandboxes" => c.check(client.sandboxes().create(&c.body()?, &c.key()?).await),
        "DELETE /api/v1/sandboxes/{id}" => {
            c.check(client.sandboxes().delete(c.param("id"), &c.key()?).await)
        }
        "POST /api/v1/sandboxes/{id}/pause" => {
            c.check(client.sandboxes().pause(c.param("id"), &c.key()?).await)
        }
        "POST /api/v1/sandboxes/{id}/revive" => {
            c.check(client.sandboxes().revive(c.param("id"), &c.key()?).await)
        }
        "POST /api/v1/sandboxes/{id}/service-access" => c.check(
            client
                .sandboxes()
                .grant_service_access(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/sandboxes/{id}/workspaces" => {
            c.check(client.workspaces().list_accessible(c.param("id")).await)
        }
        "POST /api/v1/sandboxes/{id}/workspaces" => c.check(
            client
                .workspaces()
                .create(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/workspaces/{id}" => {
            c.check(client.workspaces().read_accessible(c.param("id")).await)
        }
        "DELETE /api/v1/workspaces/{id}" => {
            c.check(client.workspaces().delete(c.param("id"), &c.key()?).await)
        }
        "GET /api/v1/workspaces/{id}/worktrees" => {
            c.check(client.worktrees().list_accessible(c.param("id")).await)
        }
        "GET /api/v1/worktrees/{id}" => {
            c.check(client.worktrees().read_accessible(c.param("id")).await)
        }
        "GET /api/v1/agents" => c.check(
            client
                .agents()
                .list_accessible(&AccessibleAgentListOptions {
                    organization: c.q("organization"),
                    filters: AgentListOptions {
                        status: c.tag("status")?,
                        sandbox: c.q("sandbox"),
                        workspace: c.q("workspace"),
                        page: c.page()?,
                    },
                })
                .await,
        ),
        "GET /api/v1/agents/provenance" => c.check(
            client
                .agents()
                .provenance_inventory(&AgentProvenanceInventoryOptions {
                    shared: c.q("shared").as_deref() == Some("true"),
                    scope: AccessibleAgentListOptions {
                        organization: c.q("organization"),
                        filters: AgentListOptions {
                            status: c.tag("status")?,
                            sandbox: c.q("sandbox"),
                            workspace: c.q("workspace"),
                            page: c.page()?,
                        },
                    },
                    creation_origin: c.tag("creation_origin")?,
                    channel_active: c.number::<bool>("channel_active")?,
                    channel_history: c.number::<bool>("channel_history")?,
                })
                .await,
        ),
        "POST /api/v1/workspaces/{id}/agents" => c.check(
            client
                .agents()
                .create(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "GET /api/v1/agents/{id}" => c.check(client.agents().read_accessible(c.param("id")).await),
        "GET /api/v1/agents/{id}/provenance" => {
            c.check(client.agents().provenance(c.param("id")).await)
        }
        "GET /api/v1/agents/{id}/source" => {
            c.check(client.agents().source_accessible(c.param("id")).await)
        }
        "GET /api/v1/agents/{id}/events" => {
            c.check(client.agents().events(c.param("id"), c.events()?).await)
        }
        "GET /api/v1/agents/{id}/transcript" => {
            c.check(client.agents().transcript(c.param("id")).await)
        }
        "GET /api/v1/agents/{id}/usage" => c.check(client.agents().usage(c.param("id")).await),
        "GET /api/v1/agents/{id}/resources" => {
            c.check(client.agents().resources(c.param("id")).await)
        }
        "POST /api/v1/agents/{id}/input" => c.check(
            client
                .agents()
                .input(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/agents/{id}/steer" => c.check(
            client
                .agents()
                .steer(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/agents/{id}/interrupt" => {
            c.check(client.agents().interrupt(c.param("id"), &c.key()?).await)
        }
        "POST /api/v1/agents/{id}/approvals/{request_id}" => c.check(
            client
                .agents()
                .answer_approval(c.param("id"), c.param("request_id"), &c.body()?, &c.key()?)
                .await,
        ),
        "POST /api/v1/agents/{id}/stop" => c.check(
            client
                .agents()
                .stop(c.param("id"), &c.body()?, &c.key()?)
                .await,
        ),
        "DELETE /api/v1/agents/{id}" => {
            c.check(client.agents().delete(c.param("id"), &c.key()?).await)
        }
        _ => panic!("unmapped resource operation {op}"),
    }
}

async fn profiles<C: ProfileContent + PartialEq + Debug>(
    c: &Context<'_>,
    profiles: Profiles<'_, C>,
) -> Result<()> {
    let (method, path) = c.case.operation.split_once(' ').unwrap();
    let tail = path
        .strip_prefix("/api/v1/")
        .unwrap()
        .split_once('/')
        .map_or("", |(_, tail)| tail);
    match (method, tail) {
        ("GET", "") => c.check(profiles.list(&c.page()?).await),
        ("POST", "") => c.check(profiles.create(&c.body()?, &c.key()?).await),
        ("GET", "{id}") => c.check(profiles.read(c.param("id")).await),
        ("PUT", "{id}") => c.check(profiles.save(c.param("id"), &c.body()?, &c.key()?).await),
        ("PUT", "{id}/visibility") => c.check(
            profiles
                .set_visibility(c.param("id"), c.field("visibility")?, &c.key()?)
                .await,
        ),
        ("POST", "{id}/revisions") => c.check(
            profiles
                .publish(c.param("id"), c.field("draft_version")?, &c.key()?)
                .await,
        ),
        ("GET", "{id}/revisions") => {
            c.check(profiles.list_revisions(c.param("id"), &c.page()?).await)
        }
        ("GET", "{id}/revisions/{revision}") => c.check(
            profiles
                .read_revision(
                    c.param("id"),
                    c.param("revision")
                        .parse()
                        .map_err(|_| invalid("unrepresentable revision"))?,
                )
                .await,
        ),
        ("POST", "{id}/retire") => c.check(profiles.retire(c.param("id"), &c.key()?).await),
        _ => panic!("unmapped profile operation {}", c.case.operation),
    }
}

async fn marketplace<C: ProfileContent + PartialEq + Debug>(c: &Context<'_>) -> Result<()> {
    let marketplace = c.client.marketplace::<C>();
    match c.case.operation.as_str() {
        "GET /api/v1/marketplace/{kind}" => c.check(
            marketplace
                .list(&MarketplaceListOptions {
                    q: c.q("q"),
                    page: c.page()?,
                })
                .await,
        ),
        "GET /api/v1/marketplace/{kind}/{id}" => c.check(marketplace.read(c.param("id")).await),
        "POST /api/v1/marketplace/{kind}/{id}/fork" => c.check(
            marketplace
                .fork(c.param("id"), c.field("revision")?, &c.key()?)
                .await,
        ),
        _ => panic!("unmapped marketplace operation {}", c.case.operation),
    }
}
