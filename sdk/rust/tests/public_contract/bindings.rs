use super::context::Context;
use cma_sdk::Result;
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let bindings = c.client.provider_bindings();
    let id = c.param("binding_id");
    match c.case.operation.as_str() {
        "PUT /api/v1/cmaeg/bindings/{binding_id}" => c.check(bindings.put(id, &c.body()?).await),
        "GET /api/v1/cmaeg/bindings/{binding_id}" => c.check(bindings.get(id).await),
        "DELETE /api/v1/cmaeg/bindings/{binding_id}" => c.check(bindings.delete(id).await),
        "PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}" => c.check(
            bindings
                .ensure_conversation(id, c.param("conversation_id"))
                .await,
        ),
        "DELETE /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}" => c.check(
            bindings
                .delete_conversation(id, c.param("conversation_id"))
                .await,
        ),
        "GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}" => {
            c.check(
                c.client
                    .responses_v2()
                    .read_event_context(id, c.param("conversation_id"), c.param("event_id"))
                    .await,
            )
        }
        op => panic!("unmapped binding operation {op}"),
    }
}
