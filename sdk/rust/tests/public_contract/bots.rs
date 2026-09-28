use super::context::{Context, invalid};
use cma_sdk::{Result, bots::BotListOptions};
use serde::Serialize;

fn check<T: Serialize>(c: &Context<'_>, result: Result<T>) -> Result<()> {
    // Bot responses intentionally omit Debug because channel references are opaque.
    c.check(result.map(|value| serde_json::to_value(value).unwrap()))
}
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let bots = c.client.bots();
    match c.case.operation.as_str() {
        "POST /api/v1/bot-setups/advance" => {
            check(c, c.client.bot_setups().advance(&c.body()?).await)
        }
        "PUT /api/v1/gateway/interaction-records/{record_id}" => c.check(
            c.client
                .gateway_interactions()
                .put(c.param("record_id"), &c.body()?)
                .await,
        ),
        "GET /api/v1/gateway/interaction-records/{record_id}" => c.check(
            c.client
                .gateway_interactions()
                .get(c.param("record_id"))
                .await,
        ),
        "DELETE /api/v1/gateway/interaction-records/{record_id}" => c.check(
            c.client
                .gateway_interactions()
                .delete(
                    c.param("record_id"),
                    c.query["expected_version"].parse().unwrap(),
                )
                .await,
        ),
        "POST /api/v1/bots" => check(c, bots.create(&c.body()?, &c.key()?).await),
        "GET /api/v1/bots" => check(
            c,
            bots.list(&BotListOptions {
                after: c.query.get("after").cloned(),
                limit: c.query.get("limit").map(|v| v.parse().unwrap()),
            })
            .await,
        ),
        "GET /api/v1/bots/nyxid-bots" => check(c, bots.available().await),
        "GET /api/v1/bots/{bot_id}" => check(c, bots.get(c.param("bot_id")).await),
        "POST /api/v1/bots/{bot_id}/test" => check(c, bots.test(c.param("bot_id")).await),
        "POST /api/v1/bots/{bot_id}/disable" => check(c, bots.disable(c.param("bot_id")).await),
        "POST /api/v1/bots/{bot_id}/enable" => check(c, bots.enable(c.param("bot_id")).await),
        "POST /api/v1/bots/{bot_id}/rotate-keys" => {
            check(c, bots.rotate_keys(c.param("bot_id")).await)
        }
        "DELETE /api/v1/bots/{bot_id}" => check(c, bots.delete(c.param("bot_id")).await),
        _ => Err(invalid("missing Bot contract dispatcher")),
    }
}
