use super::*;
use axum::{http::HeaderMap, routing::post};
use layer2_core::sandbox::binding::{
    BindingPreparationEvent, BindingPreparationInspection, BindingPreparationIntent,
};

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectPreparationRequest {
    pub schema_version: u16,
    pub admission_id: String,
    pub target_id: String,
    pub request_hash: String,
    pub owner: String,
    pub admitted_at: i64,
    pub expires_at: i64,
    pub agent_key: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareBindingRequest {
    pub schema_version: u16,
    #[schema(value_type = PreparationIntentSchema)]
    pub intent: BindingPreparationIntent,
    pub agent_key: String,
}

/// Exact scalar wire schema of the core intent, whose validated domain types
/// remain independent of OpenAPI and the public transport crate.
#[allow(dead_code)]
#[derive(ToSchema)]
pub struct PreparationIntentSchema {
    version: u8,
    admission_id: String,
    target_id: String,
    request_hash: String,
    binding_id: String,
    owner: String,
    manager: String,
    generation: u64,
    binding_revision: u64,
    binding_digest: String,
    sandbox_digest: String,
    workspace_digest: String,
    admitted_at: i64,
    expires_at: i64,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PreparationInspectionView {
    Pinned {
        version: u8,
        binding_id: String,
        generation: u64,
    },
    Pending {
        #[schema(value_type = PreparationIntentSchema)]
        intent: Box<BindingPreparationIntent>,
    },
}

#[derive(Serialize, ToSchema)]
pub struct PreparationReceiptView {
    pub version: u8,
    pub operation_id: String,
    pub intent_fingerprint: String,
    pub generation: u64,
    pub sandbox_id: String,
    pub workspace_id: String,
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(inspect, prepare),
    components(schemas(
        InspectPreparationRequest,
        PrepareBindingRequest,
        PreparationIntentSchema,
        PreparationInspectionView,
        PreparationReceiptView,
    ))
)]
struct PreparationApiDoc;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    <PreparationApiDoc as utoipa::OpenApi>::openapi()
}

pub(super) fn routes() -> Router<BindingRouteState> {
    Router::new()
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/preparation-inspections",
            post(inspect),
        )
        .route(
            "/api/v1/cmaeg/bindings/{binding_id}/preparations",
            post(prepare),
        )
}

#[utoipa::path(post, path = "/api/v1/cmaeg/bindings/{binding_id}/preparation-inspections", tag = "provider bindings",
    params(("binding_id" = String, Path)), request_body = InspectPreparationRequest,
    responses((status = 200, body = PreparationInspectionView), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
pub(crate) async fn inspect(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    body: Result<Json<InspectPreparationRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(value)) => value,
        Err(error) => return super::super::BodyProblem::from(error).into_response(),
    };
    if body.schema_version != 1 {
        return error(BindingError::InvalidRequest(
            "unsupported preparation version".into(),
        ));
    }
    let credential = match ProvidedCredential::new(body.agent_key) {
        Ok(value) => value,
        Err(_) => return error(BindingError::InvalidRequest("invalid agent key".into())),
    };
    let event = BindingPreparationEvent {
        admission_id: body.admission_id,
        target_id: body.target_id,
        request_hash: body.request_hash,
        owner: layer2_core::SubjectId::new(body.owner),
        admitted_at: body.admitted_at,
        expires_at: body.expires_at,
    };
    match state
        .service
        .inspect_preparation(&caller, &id, &event, &credential)
        .await
    {
        Ok(BindingPreparationInspection::Current {
            binding_id,
            generation,
        }) => Json(PreparationInspectionView::Pinned {
            version: 1,
            binding_id,
            generation,
        })
        .into_response(),
        Ok(BindingPreparationInspection::Retired { intent }) => {
            Json(PreparationInspectionView::Pending { intent }).into_response()
        }
        Err(value) => error(value),
    }
}

#[utoipa::path(post, path = "/api/v1/cmaeg/bindings/{binding_id}/preparations", tag = "provider bindings",
    params(("binding_id" = String, Path), ("Idempotency-Key" = String, Header)), request_body = PrepareBindingRequest,
    responses((status = 200, body = PreparationReceiptView), (status = 400), (status = 401), (status = 403), (status = 404), (status = 409), (status = 503)))]
pub(crate) async fn prepare(
    State(state): State<BindingRouteState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<PrepareBindingRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(value)) => value,
        Err(error) => return super::super::BodyProblem::from(error).into_response(),
    };
    if caller
        .require(layer2_core::Capability::ManageGatewayRecords)
        .is_err()
        || caller
            .require(layer2_core::Capability::ManageProviderBindings)
            .is_err()
    {
        return error(BindingError::Forbidden);
    }
    if !state.preparation_enabled {
        return error(BindingError::Unavailable(
            "binding preparation is disabled".into(),
        ));
    }
    let operation = body.intent.operation_id();
    if body.schema_version != 1
        || headers.get_all("idempotency-key").iter().count() != 1
        || operation.as_ref().ok().map(String::as_str)
            != headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
    {
        return error(BindingError::InvalidRequest(
            "preparation requires its exact operation key".into(),
        ));
    }
    let credential = match ProvidedCredential::new(body.agent_key) {
        Ok(value) => value,
        Err(_) => return error(BindingError::InvalidRequest("invalid agent key".into())),
    };
    match state
        .service
        .prepare_retired_binding(&caller, &id, &body.intent, &credential)
        .await
    {
        Ok(receipt) => match receipt.intent.fingerprint() {
            Ok(fingerprint) => Json(PreparationReceiptView {
                version: 1,
                operation_id: receipt.operation_id,
                intent_fingerprint: fingerprint,
                generation: receipt.reserved.generation,
                sandbox_id: receipt.reserved.sandbox_id.to_string(),
                workspace_id: receipt.reserved.workspace_id.to_string(),
            })
            .into_response(),
            Err(_) => error(BindingError::Corrupt("invalid preparation receipt".into())),
        },
        Err(value) => error(value),
    }
}

/// A single canonical integer is required; duplicate or combined headers are
/// never silently accepted as an unpinned legacy request.
pub(super) fn generation_header(headers: &HeaderMap) -> Result<Option<u64>, BindingError> {
    let mut values = headers.get_all("x-cma-binding-generation").iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let generation = value.to_str().ok().and_then(|value| {
        value
            .parse::<u64>()
            .ok()
            .filter(|generation| *generation > 0 && generation.to_string() == value)
    });
    if values.next().is_some() || generation.is_none() {
        return Err(BindingError::InvalidRequest(
            "invalid binding generation header".into(),
        ));
    }
    Ok(generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_header_never_downgrades_invalid_or_duplicate_pins() {
        let mut headers = HeaderMap::new();
        assert_eq!(generation_header(&headers).unwrap(), None);
        for value in ["0", "01", "-1", "+1", "1,2", "18446744073709551616", " 1"] {
            headers.insert("x-cma-binding-generation", value.parse().unwrap());
            assert!(generation_header(&headers).is_err(), "{value}");
        }
        headers.insert("x-cma-binding-generation", "2".parse().unwrap());
        assert_eq!(generation_header(&headers).unwrap(), Some(2));
        headers.append("x-cma-binding-generation", "2".parse().unwrap());
        assert!(generation_header(&headers).is_err());
    }
}
