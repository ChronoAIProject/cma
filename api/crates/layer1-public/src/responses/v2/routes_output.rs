use super::*;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(in crate::responses::v2) struct OutputQuery {
    after: Option<String>,
}

#[utoipa::path(get, path = "/api/v2/responses/{response_id}/output", tag = "responses v2",
    params(("response_id" = String, Path), ("after" = Option<String>, Query)), responses((status = 200, body = Value)))]
pub(in crate::responses::v2) async fn output_page(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(id): Path<String>,
    query: Result<Query<OutputQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    let Ok(Query(query)) = query else {
        return error::field(
            "after",
            "invalid_output_cursor",
            "Use the returned output-page cursor.",
        );
    };
    let (authorization, receipt) =
        match authorize(&state, &auth, &AgentResponseId(id.clone())).await {
            Ok(value) => value,
            Err(value) => return *value,
        };
    let reading = match state
        .commands
        .response_for_stream(&authorization, &AgentResponseId(id.clone()))
        .await
    {
        Ok(value) if value.command.receipt == receipt => value,
        _ => return error::unavailable(),
    };
    let cursor = query.after.clone();
    let page=state.reads.with_output_observation(&authorization,reading.command.turn_sequence,move |source|Box::pin(async move {
        let from=match cursor {
            None=>0,Some(cursor)=>cursor.strip_prefix("indexed:").and_then(|s|s.parse::<u64>().ok()).ok_or(layer2_core::sandbox::AgentReadError::InvalidPage)?,
        };
        let through=source.metadata().through;
        if source.metadata().index.is_none() {
            if from!=0 {return Err(layer2_core::sandbox::AgentReadError::InvalidPage);}
            return Ok(serde_json::json!({"data":[],"has_more":false,"next_after":null,"through":through}));
        }
        let page=super::super::projection::indexed::output_page(source,from).await?;
        Ok(serde_json::json!({"data":page.items,"has_more":page.more,"next_after":page.more.then(||format!("indexed:{}",page.next)),"through":through}))
    })).await;
    let mut body = match page {
        Ok(Some(value)) => value,
        Ok(None) => {
            let from = match query.after {
                None => 0,
                Some(cursor) => match cursor
                    .strip_prefix("legacy:")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    Some(value) => value,
                    None => {
                        return error::field(
                            "after",
                            "invalid_output_cursor",
                            "The cursor does not match this source format.",
                        )
                    }
                },
            };
            let (response, _) =
                match super::super::stream::snapshot(&state, &authorization, &receipt).await {
                    Ok(value) => value,
                    Err(error) => return error,
                };
            if from > response.output.len() {
                return error::field(
                    "after",
                    "invalid_output_cursor",
                    "The cursor is beyond the output.",
                );
            }
            let next = response.output.len().min(from.saturating_add(1));
            if serde_json::to_vec(&response.output[from..next]).map_or(true, |b| {
                b.len() + 2048 > layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES
            }) {
                return error::field(
                    "output",
                    "response_output_item_too_large",
                    "One output item exceeds the page budget.",
                );
            }
            serde_json::json!({"data":&response.output[from..next],"has_more":next<response.output.len(),"next_after":(next<response.output.len()).then(||format!("legacy:{next}"))})
        }
        Err(layer2_core::sandbox::AgentReadError::InvalidPage) => {
            return error::field(
                "after",
                "invalid_output_cursor",
                "Use a cursor from this response and source format.",
            )
        }
        Err(value) => return error::read(value),
    };
    body["object"] = "cma.response.output_page".into();
    body["response_id"] = id.into();
    Json(body).into_response()
}
