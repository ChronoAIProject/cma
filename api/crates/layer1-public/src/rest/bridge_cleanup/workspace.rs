//! Separately authorized removal of the originally owned Workspace.
use super::workspace_shapes::*;
use super::*;

fn respond<T: Serialize>(status: StatusCode, result: Result<T, CleanupError>) -> Response {
    match result {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => problem::WorkspaceProblem(error).into_response(),
    }
}
fn unavailable() -> Response {
    problem::WorkspaceProblem(CleanupError::Unavailable).into_response()
}
#[utoipa::path(get,path="/api/v1/bridges/{id}/allocations/{allocation}/workspace-cleanup",tag="bridges",params(("id"=String,Path),("allocation"=String,Path)),responses((status=200,body=WorkspaceCleanupStatusShape),(status=400,body=super::super::ProblemDocument),(status=403,body=super::super::ProblemDocument),(status=404,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn detail(
    State(service): State<Option<Arc<BridgeCleanupService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation)): Path<(String, String)>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    respond(
        StatusCode::OK,
        service
            .workspace_status(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                &allocation,
            )
            .await,
    )
}
#[utoipa::path(post,path="/api/v1/bridges/{id}/allocations/{allocation}/workspace-cleanup",tag="bridges",params(("id"=String,Path),("allocation"=String,Path),("Idempotency-Key"=String,Header)),request_body=WorkspaceCleanupRequestShape,responses((status=202,body=WorkspaceCleanupStatusShape),(status=400,body=super::super::ProblemDocument),(status=403,body=super::super::ProblemDocument),(status=404,body=super::super::ProblemDocument),(status=409,body=super::super::ProblemDocument),(status=415,body=super::super::ProblemDocument),(status=422,body=super::super::ProblemDocument),(status=503,body=super::super::ProblemDocument)))]
pub(crate) async fn submit(
    State(service): State<Option<Arc<BridgeCleanupService>>>,
    caller: AuthenticatedResourceCaller,
    Path((id, allocation_id)): Path<(String, String)>,
    key: IdempotencyKey,
    input: Result<RequestBody<WorkspaceCleanupRequest>, BodyProblem>,
) -> Response {
    let Some(service) = service else {
        return unavailable();
    };
    let idempotency_key = match key.into_required_value() {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let request = match input.and_then(|body| body.into_parts().0) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    respond(
        StatusCode::ACCEPTED,
        service
            .workspace_submit(
                &caller.context,
                credential(&caller),
                &BridgeId(id),
                WorkspaceCleanupCommand {
                    allocation_id,
                    idempotency_key,
                    request,
                },
            )
            .await,
    )
}
