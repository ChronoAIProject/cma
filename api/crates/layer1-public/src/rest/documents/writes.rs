use super::*;
use axum::extract::Path;
use layer2_core::document_artifacts as core;

macro_rules! request_parts {
    ($key:ident, $body:ident) => {
        let $key = match required_key($key) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        let $body = match json_body($body) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
    };
}
#[utoipa::path(post,path="/api/v1/documents",tag="documents",params(("Idempotency-Key"=String,Header)),request_body=CreateDocumentRequest,responses((status=201,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn create(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    key: IdempotencyKey,
    body: Result<RequestBody<core::CreateDocument>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service.create(&caller, &key, body).await,
        StatusCode::CREATED,
    )
}
#[utoipa::path(patch,path="/api/v1/documents/{id}",tag="documents",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=UpdateDocumentRequest,responses((status=200,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn update(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<core::UpdateDocument>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service.update(&caller, &id, &key, body).await,
        StatusCode::OK,
    )
}
#[utoipa::path(delete,path="/api/v1/documents/{id}",tag="documents",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=DocumentRevisionRequest,responses((status=200,body=DocumentDeletionReceipt),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=403,description="Account deletion in progress",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn delete(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<DocumentRevisionRequest>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service
            .delete(&caller, &id, &key, body.expected_revision)
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(put,path="/api/v1/documents/{id}/shares/{subject}",tag="documents",params(("id"=String,Path),("subject"=String,Path),("Idempotency-Key"=String,Header)),request_body=GrantDocumentRequest,responses((status=200,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn grant(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, subject)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<core::GrantDocument>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service.grant(&caller, &id, &key, &subject, body).await,
        StatusCode::OK,
    )
}
#[utoipa::path(delete,path="/api/v1/documents/{id}/shares/{subject}",tag="documents",params(("id"=String,Path),("subject"=String,Path),("Idempotency-Key"=String,Header)),request_body=DocumentRevisionRequest,responses((status=200,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn revoke(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, subject)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<DocumentRevisionRequest>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service
            .revoke(&caller, &id, &key, &subject, body.expected_revision)
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/api/v1/documents/{id}/share-links",tag="documents",params(("id"=String,Path),("Idempotency-Key"=String,Header)),request_body=CreateDocumentLinkRequest,responses((status=201,body=DocumentMutation),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn create_link(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    key: IdempotencyKey,
    body: Result<RequestBody<core::CreateDocumentLink>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service.create_link(&caller, &id, &key, body).await,
        StatusCode::CREATED,
    )
}
#[utoipa::path(delete,path="/api/v1/documents/{id}/share-links/{link_id}",tag="documents",params(("id"=String,Path),("link_id"=String,Path),("Idempotency-Key"=String,Header)),request_body=DocumentRevisionRequest,responses((status=200,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn revoke_link(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, link_id)): Path<(String, String)>,
    key: IdempotencyKey,
    body: Result<RequestBody<DocumentRevisionRequest>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    result(
        service
            .revoke_link(&caller, &id, &key, &link_id, body.expected_revision)
            .await,
        StatusCode::OK,
    )
}
#[utoipa::path(post,path="/api/v1/documents/{id}/comments",tag="documents",params(("id"=String,Path),("Idempotency-Key"=String,Header),("X-CMA-Document-Share-Token"=Option<String>,Header)),request_body=CreateDocumentCommentRequest,responses((status=201,body=DocumentMutation),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn comment(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    key: IdempotencyKey,
    body: Result<RequestBody<core::CreateDocumentComment>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    let token = match share_token(&headers) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    result(
        service.comment(&caller, &id, token, &key, body).await,
        StatusCode::CREATED,
    )
}
#[utoipa::path(delete,path="/api/v1/documents/{id}/comments/{comment_id}",tag="documents",params(("id"=String,Path),("comment_id"=String,Path),("Idempotency-Key"=String,Header),("X-CMA-Document-Share-Token"=Option<String>,Header)),request_body=DocumentRevisionRequest,responses((status=200,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=409,description="Revision or idempotency conflict",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=429,description="Bounded document capacity reached",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=413,description="JSON body exceeds byte budget",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=415,description="JSON content type required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=422,description="JSON does not match the command",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn remove_comment(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path((id, comment_id)): Path<(String, String)>,
    headers: HeaderMap,
    key: IdempotencyKey,
    body: Result<RequestBody<DocumentRevisionRequest>, super::super::BodyProblem>,
) -> Response {
    request_parts!(key, body);
    let token = match share_token(&headers) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    result(
        service
            .remove_comment(
                &caller,
                &id,
                token,
                &key,
                &comment_id,
                body.expected_revision,
            )
            .await,
        StatusCode::OK,
    )
}
