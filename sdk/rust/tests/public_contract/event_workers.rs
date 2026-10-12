use super::context::{Context, invalid};
use cma_sdk::{Result, event_worker_profiles::EventWorkerProfileListOptions};

pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let profiles = c.client.event_worker_profiles();
    match c.case.operation.as_str() {
        "POST /api/v1/event-worker-profiles" => {
            check(c, profiles.create(&c.body()?, &c.key()?).await)
        }
        "GET /api/v1/event-worker-profiles" => check(
            c,
            profiles
                .list(&EventWorkerProfileListOptions {
                    after: c.query.get("after").cloned(),
                    limit: c.query.get("limit").map(|v| v.parse().unwrap()),
                })
                .await,
        ),
        "GET /api/v1/event-worker-profiles/{id}" => check(c, profiles.get(c.param("id")).await),
        "GET /api/v1/event-worker-profiles/{id}/revisions/{revision}" => check(
            c,
            profiles
                .revision(c.param("id"), c.param("revision").parse().unwrap())
                .await,
        ),
        "PUT /api/v1/event-worker-profiles/{id}" => {
            check(c, profiles.update(c.param("id"), &c.body()?).await)
        }
        "DELETE /api/v1/event-worker-profiles/{id}" => check(
            c,
            profiles
                .retire(c.param("id"), c.query["expected_revision"].parse().unwrap())
                .await,
        ),
        _ => Err(invalid("missing Event Worker Profile contract dispatcher")),
    }
}

fn check<T: serde::Serialize>(c: &Context<'_>, result: Result<T>) -> Result<()> {
    c.check(result.map(|value| serde_json::to_value(value).unwrap()))
}
