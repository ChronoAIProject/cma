//! Additive preparation/status routes; no request to these routes sends a message.
use crate::{
    auth::AgentCaller,
    tools::{FormPreparationReceipt, ToolsState},
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::post,
    Json, Router,
};
use cmaeg_core::{
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
};

pub(crate) fn routes() -> Router<ToolsState> {
    Router::new().route(
        "/v1/events/{event_ref}/form-preparation",
        post(prepare_form_reply).get(read_form_preparation),
    )
}
#[utoipa::path(post,path="/v1/events/{event_ref}/form-preparation",operation_id="prepareFormReply",tag="tools",
security(("agentBearer"=[])),params(("event_ref"=String,Path),("Idempotency-Key"=String,Header,description="Required stable preparation key; preserve the same key and exact message after a timeout.")),request_body=OutboundMessage,
responses((status=200,body=FormPreparationReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn prepare_form_reply(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    headers: HeaderMap,
    body: Result<Json<OutboundMessage>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let result = async {
        if headers.contains_key("form-preparation-ref") {
            return Err(invalid());
        }
        let message = super::tools::json(body)?;
        let event = super::tools::event_ref(&reference)?;
        let key = super::tools::reply_key(&headers)?.ok_or_else(invalid)?;
        state.prepare_form_reply(caller, &event, message, key).await
    }
    .await;
    super::tools::result(result)
}
#[utoipa::path(get,path="/v1/events/{event_ref}/form-preparation",operation_id="readFormPreparation",tag="tools",
security(("agentBearer"=[])),params(("event_ref"=String,Path)),
responses((status=200,body=FormPreparationReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read_form_preparation(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(reference): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        super::tools::reply_key(&headers)?;
        if headers.contains_key("form-preparation-ref") {
            return Err(invalid());
        }
        state
            .read_form_preparation(caller, &super::tools::event_ref(&reference)?)
            .await
    }
    .await;
    super::tools::result(result)
}
pub(crate) fn selector(headers: &HeaderMap) -> Result<Option<&str>, Problem> {
    let mut values = headers.get_all("form-preparation-ref").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| invalid())?;
    if values.next().is_some() || uuid::Uuid::parse_str(value).is_err() {
        return Err(invalid());
    }
    Ok(Some(value))
}
fn invalid() -> Problem {
    Problem::new(
        ProblemCode::InvalidRequest,
        400,
        "one_valid_form_preparation_ref_and_required_idempotency_key",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_even_identical_keys_and_selectors_are_refused() {
        let mut headers = HeaderMap::new();
        headers.append("idempotency-key", "stable".parse().unwrap());
        headers.append("idempotency-key", "stable".parse().unwrap());
        assert!(crate::http::tools::reply_key(&headers).is_err());
        let mut headers = HeaderMap::new();
        let reference = uuid::Uuid::new_v4().to_string();
        headers.append("form-preparation-ref", reference.parse().unwrap());
        headers.append("form-preparation-ref", reference.parse().unwrap());
        assert!(selector(&headers).is_err());
    }
}
