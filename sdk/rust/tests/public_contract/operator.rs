use super::context::{Context, decode};
use cma_sdk::{
    Result,
    admin::{ActivityListOptions, ProfileListOptions, SandboxListOptions},
    profile_authoring::ProfileAuthoringSkillSearch,
};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let admin = c.client.admin();
    match c.case.operation.as_str() {
        "POST /api/v1/telemetry/browser" => {
            let body: serde_json::Value = c.body()?;
            c.check(
                c.client
                    .report_browser_fault(serde_json::from_value(body["kind"].clone()).unwrap())
                    .await,
            )
        }
        "GET /api/v1/conformance" => c.check(c.client.read_conformance().await),
        "GET /api/v1/defaults" => c.check(c.client.read_defaults().await),
        "GET /openapi.json" => c.check(c.client.read_openapi().await),
        "GET /api/v1/profile-authoring/bounds" => {
            c.check(c.client.profile_authoring().bounds().await)
        }
        "GET /api/v1/profile-authoring/services" => {
            c.check(c.client.profile_authoring().services().await)
        }
        "GET /api/v1/profile-authoring/skills" => c.check(
            c.client
                .profile_authoring()
                .search_skills(&ProfileAuthoringSkillSearch {
                    q: c.q("q"),
                    limit: c.number("limit")?,
                    cursor: c.q("cursor"),
                })
                .await,
        ),
        "POST /api/v1/profile-authoring/repositories/resolve" => c.check(
            c.client
                .profile_authoring()
                .resolve_repository(&c.body()?)
                .await,
        ),
        "GET /api/v1/admin/admission" => c.check(admin.read_admission().await),
        "POST /api/v1/admin/admission/stop" => c.check(admin.stop_admission(&c.body()?).await),
        "POST /api/v1/admin/admission/resume" => c.check(admin.resume_admission(&c.body()?).await),
        "GET /api/v1/admin/settings" => c.check(admin.read_settings().await),
        "PUT /api/v1/admin/settings" => c.check(admin.replace_settings(&c.body()?).await),
        "PATCH /api/v1/admin/settings" => c.check(admin.patch_settings(&c.body()?).await),
        "POST /api/v1/admin/settings/reset" => {
            c.check(admin.reset_settings(c.field("expected_revision")?).await)
        }
        "GET /api/v1/admin/auto-pause/preview" => c.check(admin.preview_auto_pause().await),
        "POST /api/v1/admin/auto-pause/pause-idle-now" => {
            c.check(admin.pause_idle_now(&c.body()?).await)
        }
        "POST /api/v1/admin/sandboxes/{id}/always-on" => c.check(
            admin
                .set_sandbox_always_on(c.param("id"), &c.body::<cma_sdk::admin::AlwaysOnRequest>()?)
                .await,
        ),
        "POST /api/v1/admin/triggers/pause" => c.check(admin.pause_triggers().await),
        "POST /api/v1/admin/triggers/resume" => c.check(admin.resume_triggers().await),
        "GET /api/v1/admin/sandboxes" => c.check(
            admin
                .list_sandboxes(&SandboxListOptions {
                    owner: c.q("owner"),
                    status: c.tag("status")?,
                    runtime_version: c.q("runtime_version"),
                    always_on: c.q("always_on").map(|value| value == "true"),
                    page: c.page()?,
                })
                .await,
        ),
        "GET /api/v1/admin/sandboxes/{id}" => c.check(admin.read_sandbox(c.param("id")).await),
        "GET /api/v1/admin/sandboxes/{id}/events" => {
            c.check(admin.sandbox_events(c.param("id"), c.events()?).await)
        }
        "POST /api/v1/admin/sandboxes/{id}/retire" => {
            c.check(admin.retire_sandbox(c.param("id")).await)
        }
        "POST /api/v1/admin/sandboxes/{id}/runtime-update" => c.check(
            admin
                .update_sandbox_runtime(c.param("id"), &c.body()?)
                .await,
        ),
        "GET /api/v1/admin/profiles" => c.check(
            admin
                .list_profiles(&ProfileListOptions {
                    kind: c.tag("kind")?,
                    owner: c.q("owner"),
                    visibility: c.tag("visibility")?,
                    page: c.page()?,
                })
                .await,
        ),
        "POST /api/v1/admin/profiles/{kind}/{id}/retire" => c.check(
            admin
                .retire_profile(decode(c.param("kind").into())?, c.param("id"))
                .await,
        ),
        "GET /api/v1/admin/runtime-inventory" => c.check(admin.read_runtime_inventory().await),
        "GET /api/v1/admin/warm-pools" => c.check(admin.list_warm_pools().await),
        "POST /api/v1/admin/warm-pools" => {
            c.check(admin.configure_warm_pool(&c.body()?, &c.key()?).await)
        }
        "PUT /api/v1/admin/warm-pools/{id}" => {
            c.check(admin.resize_warm_pool(c.param("id"), &c.body()?).await)
        }
        "GET /api/v1/admin/activity" => c.check(
            admin
                .list_activity(&ActivityListOptions {
                    actor: c.q("actor"),
                    action: c.tag("action")?,
                    page: c.page()?,
                })
                .await,
        ),
        "POST /api/v1/mcp" => c.check(c.client.unsupported().mcp().await),
        "POST /api/v1/a2a" => c.check(c.client.unsupported().a2a().await),
        "GET /.well-known/agent-card.json" => c.check(c.client.unsupported().agent_card().await),
        "GET /api/v1/agents/{id}/terminal" => {
            c.check(c.client.agents().terminal(c.param("id")).await)
        }
        _ => panic!("unmapped operator operation {}", c.case.operation),
    }
}
