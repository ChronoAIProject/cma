use super::*;

#[utoipa::path(post,path="/api/v1/bridges",tag="bridges",params(("Idempotency-Key"=String,Header)),request_body=CreateBridgeRequest,responses((status=201,body=BridgeReceiptShape),(status=400,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn create(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    idempotency: IdempotencyKey,
    request: Result<RequestBody<CreateBridgeRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let request = match body(request) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    let result = service
        .create(
            &caller.context,
            credential(&caller),
            key,
            CreateBridge {
                owner: request.owner,
                config: request.config,
            },
        )
        .await;
    match result {
        Ok(receipt) => {
            let location = format!("/api/v1/bridges/{}", receipt.bridge_id.as_str());
            let mut response = (StatusCode::CREATED, Json(receipt)).into_response();
            response.headers_mut().insert(
                header::LOCATION,
                location.parse().expect("generated bridge id"),
            );
            response
        }
        Err(error) => BridgeProblem(error).into_response(),
    }
}
#[utoipa::path(put,path="/api/v1/bridges/{id}/configuration",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=UpdateBridgeRequest,responses((status=200,body=BridgeReceiptShape),(status=400,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn update(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    idempotency: IdempotencyKey,
    request: Result<RequestBody<UpdateBridgeRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let request = match body(request) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    respond(
        service
            .update(
                &caller.context,
                credential(&caller),
                BridgeId(id),
                key,
                UpdateBridge {
                    expected_revision: request.expected_revision,
                    config: request.config,
                    previous_work: request.previous_work,
                },
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/commands",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=ControlBridgeRequest,responses((status=200,body=BridgeReceiptShape),(status=403,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn control(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    idempotency: IdempotencyKey,
    request: Result<RequestBody<ControlBridgeRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let request = match body(request) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    respond(
        service
            .control(
                &caller.context,
                credential(&caller),
                BridgeId(id),
                key,
                ControlBridge {
                    expected_revision: request.expected_revision,
                    command: request.command,
                },
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/validate",tag="bridges",params(("id"=String,Path)),request_body=BridgeRevisionRequest,responses((status=200,body=BridgeValidationShape),(status=403,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn validate(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    request: Result<RequestBody<BridgeRevisionRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let request = match body(request) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    respond(
        service
            .validate(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                request.expected_revision,
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(put,path="/api/v1/bridges/{id}/credential",tag="bridges",params(("id"=String,Path)),request_body=ImportBridgeKeyRequest,responses((status=200,body=BridgeCredentialStatusShape),(status=403,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn import(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    request: Result<RequestBody<ImportBridgeKeyRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let request = match body(request) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    respond(
        service
            .import(
                &caller.context,
                credential(&caller),
                BridgeCredentialImport {
                    id: BridgeId(id),
                    expected_revision: request.expected_revision,
                    material: BridgeKeyMaterial::new(request.key),
                },
            )
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/credential/revoke",tag="bridges",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=RevokeBridgeKeyRequest,responses((status=200,body=BridgeCredentialStatusShape),(status=403,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn revoke(
    State(service): State<Option<Arc<BridgeSetupService>>>,
    caller: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    idempotency: IdempotencyKey,
    request: Result<RequestBody<RevokeBridgeKeyRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let key = match key(idempotency) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let request = match body(request) {
        Ok(body) => body,
        Err(response) => return *response,
    };
    respond(
        service
            .revoke(
                &caller.context,
                credential(&caller),
                BridgeCredentialRevocation {
                    id: BridgeId(id),
                    expected_revision: request.expected_revision,
                    expected_custody_revision: request.expected_custody_revision,
                    idempotency_key: key,
                },
            )
            .await,
        StatusCode::OK,
    )
}
