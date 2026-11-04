use super::context::{Context, invalid};
use cma_sdk::Result;

pub async fn dispatch(context: &Context<'_>) -> Result<()> {
    let browser = context.client.agent_browser();
    let agent = context.param("id");
    match context.case.request.method.as_str() {
        "GET" => context.check(
            browser
                .read(
                    agent,
                    &context.q("runtime_incarnation").unwrap_or_default(),
                    context.q("observer").as_deref(),
                    context.q("context_id").as_deref(),
                )
                .await,
        ),
        "PUT" => context.check(
            browser
                .configure(
                    agent,
                    &context.field::<String>("runtime_incarnation")?,
                    context.field("enabled")?,
                )
                .await,
        ),
        "DELETE" => context.check(
            browser
                .release(
                    agent,
                    &context.q("runtime_incarnation").unwrap_or_default(),
                    &context.q("observer").unwrap_or_default(),
                )
                .await,
        ),
        _ => Err(invalid("unknown browser method")),
    }
}
