use super::context::Context;
use cma_sdk::Result;
pub async fn dispatch(c: &Context<'_>) -> Result<()> {
    let bindings = c.client.provider_bindings();
    if c.case.operation == "POST /api/v1/cmaeg/worker-credentials/classify" {
        return c.check(bindings.classify_worker_credential(&c.body()?).await);
    }
    let id = c.param("binding_id");
    if c.case.operation.contains("/media/{attachment_index}/") {
        let target = cma_sdk::provider_bindings::BindingMediaTarget {
            binding_id: id,
            conversation_id: c.param("conversation_id"),
            event_id: c.param("event_id"),
            attachment_index: c.param("attachment_index").parse().unwrap(),
        };
        return if c.case.operation.ends_with("/supplement") {
            c.check(bindings.supplement_event_media(&target, &c.body()?).await)
        } else if c.case.request.method == "GET" {
            c.check(bindings.event_attachment(&target).await)
        } else {
            let params = cma_sdk::provider_bindings::BindingMediaUpload {
                name: c.q("name").unwrap(),
                sha256: c.q("sha256").unwrap(),
                event_expires_at: c.number("event_expires_at")?.unwrap(),
                gateway_binding_generation: c.number("gateway_binding_generation")?,
                media_type: c.case.request.headers.get("content-type").unwrap().clone(),
            };
            c.check(
                bindings
                    .upload_event_attachment(
                        &target,
                        &params,
                        c.case.request.expanded_raw_body().unwrap().into_bytes(),
                    )
                    .await,
            )
        };
    }
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
        "GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}" => c.check(
            bindings
                .inspect_conversation(id, c.param("conversation_id"))
                .await,
        ),
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
