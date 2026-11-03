use super::context::{Context, invalid};
use cma_sdk::{
    Result,
    agent_browser_control::{BrowserControlChange, BrowserControlScope},
};

pub async fn dispatch(context: &Context<'_>) -> Result<()> {
    let browser = context.client.agent_browser_control();
    let agent = context.param("id");
    let key = context
        .case
        .request
        .headers
        .get("x-cma-browser-client")
        .cloned()
        .unwrap_or_default();
    match context.case.request.method.as_str() {
        "GET" => context.check(
            browser
                .read(
                    agent,
                    &BrowserControlScope {
                        runtime_incarnation: context.q("runtime_incarnation").unwrap_or_default(),
                        context_id: context.q("context_id").unwrap_or_default(),
                        client_id: context.q("client_id").unwrap_or_default(),
                        client_key: key,
                        operation_id: context.q("operation_id"),
                    },
                )
                .await,
        ),
        "POST" => {
            let mut command: BrowserControlChange = context.body()?;
            command.scope.client_key = key;
            context.check(browser.change(agent, &command).await)
        }
        _ => Err(invalid("unknown browser control method")),
    }
}
