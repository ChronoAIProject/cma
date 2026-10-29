use super::context::Context;
use cma_sdk::Result;
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let bindings = c.client.provider_bindings();
    let id = c.param("binding_id");
    match c.case.operation.as_str() {
        "POST /api/v1/cmaeg/bindings/{binding_id}/preparation-inspections" => {
            c.check(bindings.inspect_preparation(id, &c.body()?).await)
        }
        "POST /api/v1/cmaeg/bindings/{binding_id}/preparations" => {
            c.check(bindings.prepare(id, &c.body()?, &c.key()?).await)
        }
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
