//! Common admission problems are additive; ProblemDocument.code remains open.
pub(super) fn describe(document: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::{ContentBuilder, Ref, ResponseBuilder};
    document
        .extensions
        .get_or_insert_with(Default::default)
        .insert(
            "x-cma-capacity-problems".into(),
            serde_json::json!([
                "runtime_request_capacity_exhausted",
                "runtime_stream_capacity_exhausted",
                "relay_observer_capacity_exhausted",
                "relay_capacity_exhausted"
            ]),
        );
    for (path, item) in &mut document.paths.paths {
        if !(path.starts_with("/api/v1/agents/")
            || path.starts_with("/api/v1/agui/")
            || path.starts_with("/api/v1/responses")
            || (path.starts_with("/api/v1/cmaeg/bindings/") && path.ends_with("/attachment"))
            || path == "/api/v2/agents/{agent_id}/events")
        {
            continue;
        }
        for operation in [
            item.get.as_mut(),
            item.post.as_mut(),
            item.put.as_mut(),
            item.delete.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            operation.responses.responses.entry("429".into()).or_insert_with(|| {
                ResponseBuilder::new()
                    .description("Bounded admission capacity exhausted; named problem code and Retry-After: 1. Existing work continues; retain the original operation identity and replay cursor.")
                    .content("application/problem+json", ContentBuilder::new().schema(Some(Ref::from_schema_name("ProblemDocument"))).build())
                    .build().into()
            });
        }
    }
}
