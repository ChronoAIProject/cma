use super::context::Context;
use cma_sdk::Result;

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let usage = c.client.usage();
    match c.case.operation.as_str() {
        "GET /api/v1/usage" => c.check(usage.summary().await),
        "GET /api/v1/usage/documents" => c.check(usage.documents().await),
        "GET /api/v1/usage/sandboxes" => c.check(usage.sandboxes(&c.page()?).await),
        "GET /api/v1/usage/sandboxes/{id}" => c.check(usage.sandbox(c.param("id")).await),
        "GET /api/v1/usage/sandboxes/{id}/events" => {
            c.check(usage.events(c.param("id"), &c.page()?).await)
        }
        "GET /api/v1/admin/usage" => c.check(usage.admin_summary(c.q("owner").as_deref()).await),
        "GET /api/v1/admin/usage/sandboxes" => c.check(
            usage
                .admin_sandboxes(c.q("owner").as_deref(), &c.page()?)
                .await,
        ),
        operation => panic!("unhandled usage operation {operation}"),
    }
}
