use super::context::Context;
use cma_sdk::{
    Result,
    usage_metrics::{PageOptions, SeriesOptions},
};
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let api = if c.case.operation.contains("/admin/") {
        c.client.admin_usage_metrics()
    } else {
        c.client.usage_metrics()
    };
    let op = c.case.operation.as_str();
    if op == "POST /api/v1/admin/usage/metrics/options" {
        return c.check(api.options(&c.body()?).await);
    }
    if op.starts_with("POST ") {
        return c.check(api.create(&c.body()?).await);
    }
    let id = c.param("report");
    let page = PageOptions {
        limit: c.number("limit")?,
        cursor: c.q("cursor"),
        sort: c.q("sort"),
        order: c.q("order"),
    };
    if op.ends_with("/owner-allocations") {
        c.check(
            api.owner_allocations(
                id,
                &cma_sdk::usage_metrics::OwnerAllocationQuery {
                    metric: c.q("metric").unwrap(),
                    limit: page.limit,
                    cursor: page.cursor,
                },
            )
            .await,
        )
    } else if op.ends_with("/owners") {
        c.check(api.owners(id, c.q("search").as_deref(), &page).await)
    } else if op.ends_with("/sandboxes") {
        c.check(api.sandboxes(id, &page).await)
    } else if op.ends_with("/{sandbox}") {
        c.check(api.sandbox(id, c.param("sandbox")).await)
    } else if op.ends_with("/series") {
        c.check(
            api.series(
                id,
                &SeriesOptions {
                    metric: c.q("metric").unwrap(),
                    group_by: c.q("group_by"),
                    series_id: c.q("series_id"),
                    top: c.number("top")?,
                },
            )
            .await,
        )
    } else {
        c.check(api.report(id).await)
    }
}
