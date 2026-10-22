use super::*;
use axum::extract::{Path, Query};
use layer2_core::document_artifacts as core;
use serde::Deserialize;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageParams {
    limit: Option<u16>,
    cursor: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceParams {
    revision: Option<u64>,
}

#[utoipa::path(get,path="/api/v1/documents",tag="documents",params(("limit"=Option<u16>,Query),("cursor"=Option<String>,Query),("scope"=Option<String>,Query),("q"=Option<String>,Query)),responses((status=200,body=DocumentPage),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn list(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    query: Result<Query<core::DocumentQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<core::DocumentPage<core::DocumentView>>, DocumentProblem> {
    let Query(query) = query.map_err(|_| DocumentProblem(DocumentError::Invalid))?;
    Ok(Json(service.list(&caller, &query).await?))
}
#[utoipa::path(get,path="/api/v1/documents/{id}",tag="documents",params(("id"=String,Path),("X-CMA-Document-Share-Token"=Option<String>,Header)),responses((status=200,body=DocumentView),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn read(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<core::DocumentView>, DocumentProblem> {
    Ok(Json(
        service.read(&caller, &id, share_token(&headers)?).await?,
    ))
}
#[utoipa::path(get,path="/api/v1/documents/{id}/source",tag="documents",params(("id"=String,Path),("revision"=Option<u64>,Query),("X-CMA-Document-Share-Token"=Option<String>,Header)),responses((status=200,body=DocumentSource),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn source(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<SourceParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<core::DocumentSource>, DocumentProblem> {
    let Query(query) = query.map_err(|_| DocumentProblem(DocumentError::Invalid))?;
    Ok(Json(
        service
            .source(&caller, &id, share_token(&headers)?, query.revision)
            .await?,
    ))
}
#[utoipa::path(get,path="/api/v1/documents/{id}/revisions",tag="documents",params(("id"=String,Path),("limit"=Option<u16>,Query),("cursor"=Option<String>,Query),("X-CMA-Document-Share-Token"=Option<String>,Header)),responses((status=200,body=DocumentRevisionPage),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn revisions(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<core::DocumentPage<core::DocumentRevision>>, DocumentProblem> {
    let Query(query) = query.map_err(|_| DocumentProblem(DocumentError::Invalid))?;
    let after = query
        .cursor
        .as_ref()
        .map(|s| {
            s.parse::<u64>()
                .map_err(|_| DocumentProblem(DocumentError::Invalid))
        })
        .transpose()?
        .unwrap_or(0);
    Ok(Json(
        service
            .revisions(
                &caller,
                &id,
                share_token(&headers)?,
                after,
                query.limit.unwrap_or(25),
            )
            .await?,
    ))
}
#[utoipa::path(get,path="/api/v1/documents/{id}/shares",tag="documents",params(("id"=String,Path)),responses((status=200,body=[DocumentShare]),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn shares(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<Vec<core::DocumentShare>>, DocumentProblem> {
    Ok(Json(service.shares(&caller, &id).await?))
}
#[utoipa::path(get,path="/api/v1/documents/{id}/share-links",tag="documents",params(("id"=String,Path)),responses((status=200,body=[DocumentLink]),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn links(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<Vec<core::DocumentLink>>, DocumentProblem> {
    Ok(Json(service.links(&caller, &id).await?))
}
#[utoipa::path(get,path="/api/v1/documents/{id}/comments",tag="documents",params(("id"=String,Path),("limit"=Option<u16>,Query),("cursor"=Option<String>,Query),("X-CMA-Document-Share-Token"=Option<String>,Header)),responses((status=200,body=DocumentCommentPage),(status=400,description="Invalid document request, JSON or idempotency key",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=401,description="Authenticated subject required",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=404,description="Document unavailable to this caller",body=super::super::ProblemDocument,content_type="application/problem+json"),(status=503,description="Document storage unavailable",body=super::super::ProblemDocument,content_type="application/problem+json")))]
pub(crate) async fn comments(
    State(service): State<Arc<DocumentService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<PageParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<core::DocumentPage<core::DocumentComment>>, DocumentProblem> {
    let Query(query) = query.map_err(|_| DocumentProblem(DocumentError::Invalid))?;
    Ok(Json(
        service
            .comments(
                &caller,
                &id,
                share_token(&headers)?,
                query.cursor.as_deref(),
                query.limit.unwrap_or(25),
            )
            .await?,
    ))
}
