use super::*;

/// Preview the stable callback address before configuring the provider route.
/// Reuse the same caller and Idempotency-Key for POST /api/v1/bridges.
#[utoipa::path(get,path="/api/v1/bridge-creation-target",tag="bridges",params(("organization"=Option<String>,Query),("Idempotency-Key"=String,Header)),responses((status=200,body=BridgeCreationTargetShape),(status=400,body=super::super::ProblemDocument),(status=401,body=super::super::ProblemDocument),(status=403,body=super::super::ProblemDocument),(status=404,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn creation_target(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    idempotency: IdempotencyKey,
    params: Result<Query<OwnerParams>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let owner = owner(&caller, params.organization);
    respond(
        service
            .creation_target(&caller.context, credential(&caller), &owner, &key)
            .await,
        StatusCode::OK,
    )
}
