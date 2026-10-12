use crate::{
    auth::{AuthError, NyxIdAuth, Principal},
    cloud::Executor,
    model::*,
    permissions as p,
    store::Store,
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct App {
    pub store: Store,
    pub auth: NyxIdAuth,
    pub executor: Executor,
}
pub fn router(state: Arc<App>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(ready))
        .route("/v1/permissions", get(permissions))
        .route("/v1/images", get(list).post(create))
        .route("/v1/images/{id}", get(read).patch(edit).delete(retire))
        .route("/v1/images/{id}/visibility", put(visibility))
        .route(
            "/v1/images/{id}/shares/{subject}",
            put(share).delete(unshare),
        )
        .route("/v1/images/{id}/builds", get(builds).post(build))
        .route("/v1/images/{id}/versions", get(versions))
        .route("/v1/images/{id}/versions/{version}", get(version))
        .route("/v1/images/{id}/resolve", post(resolve))
        .route("/v1/builds/{id}", get(build_read))
        .route("/v1/builds/{id}/logs", get(logs))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(32 * 1024))
        .with_state(state)
}
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Domain(#[from] Error),
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Auth(AuthError::Unauthenticated) => StatusCode::UNAUTHORIZED,
            Self::Auth(AuthError::Forbidden) => StatusCode::FORBIDDEN,
            Self::Domain(Error::Invalid(_)) => StatusCode::BAD_REQUEST,
            Self::Domain(Error::NotFound) => StatusCode::NOT_FOUND,
            Self::Domain(Error::Conflict) => StatusCode::CONFLICT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status,[("content-type","application/problem+json")],Json(json!({"type":"about:blank","title":status.canonical_reason(),"status":status.as_u16(),"detail":self.to_string()}))).into_response()
    }
}
type ResponseResult<T> = std::result::Result<T, HttpError>;
async fn actor(app: &App, headers: &HeaderMap, permission: &str) -> ResponseResult<Principal> {
    let principal = app.auth.authenticate(headers).await?;
    principal.require(permission)?;
    Ok(principal)
}
fn key(headers: &HeaderMap) -> ResponseResult<&str> {
    if headers.get_all("idempotency-key").iter().count() != 1 {
        return Err(Error::Invalid("one Idempotency-Key is required").into());
    }
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| bounded_text(s, 256))
        .ok_or(Error::Invalid("invalid Idempotency-Key").into())
}
fn revision(headers: &HeaderMap) -> ResponseResult<i64> {
    if headers.get_all("if-match").iter().count() != 1 {
        return Err(Error::Invalid("If-Match revision is required").into());
    }
    headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim_matches('"').parse().ok())
        .filter(|v| *v > 0)
        .ok_or(Error::Invalid("invalid If-Match revision").into())
}
fn image_view(image: Image, actor: &str) -> Value {
    let visibility = if image.visibility == ImageVisibility::Public {
        "public"
    } else if image.owner == actor {
        "private"
    } else {
        "shared"
    };
    let shares = if image.owner == actor {
        image.shares
    } else {
        vec![]
    };
    json!({"id":image.id,"owner":image.owner,"name":image.spec.name,"description":image.spec.description,"shares":shares,"visibility":visibility,"retired":image.retired,"revision":image.revision,"created_at_ms":image.created_at_ms})
}
fn build_view(build: Build) -> Value {
    json!({"id":build.id,"image_id":build.image_id,"source":build.source,"phase":build.phase,"error":build.error,"revision":build.revision,"reference":build.reference(),"digest":build.digest,"created_at_ms":build.created_at_ms,"updated_at_ms":build.updated_at_ms})
}
fn version_view(build: Build) -> Value {
    json!({"id":build.id,"image_id":build.image_id,"phase":build.phase,"reference":build.reference(),"digest":build.digest,"created_at_ms":build.created_at_ms})
}
#[derive(Default, Deserialize)]
struct Page {
    #[serde(default)]
    after: String,
    #[serde(default)]
    scope: ImageScopeParam,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ImageScopeParam {
    Private,
    Owned,
    Shared,
    Public,
    #[default]
    All,
}

impl From<ImageScopeParam> for crate::store::ImageListScope {
    fn from(value: ImageScopeParam) -> Self {
        match value {
            ImageScopeParam::Private => Self::Private,
            ImageScopeParam::Owned => Self::Owned,
            ImageScopeParam::Shared => Self::Shared,
            ImageScopeParam::Public => Self::Public,
            ImageScopeParam::All => Self::All,
        }
    }
}
fn paginated<T>(mut items: Vec<T>, id: impl Fn(&T) -> &str, view: impl Fn(T) -> Value) -> Value {
    let more = items.len() > 100;
    items.truncate(100);
    let next = if more {
        items.last().map(|x| id(x).to_owned())
    } else {
        None
    };
    json!({"items":items.into_iter().map(view).collect::<Vec<_>>(),"next_cursor":next})
}
async fn ready(State(a): State<Arc<App>>) -> ResponseResult<&'static str> {
    a.store.ping().await?;
    Ok("ok")
}
async fn permissions(State(a): State<Arc<App>>, h: HeaderMap) -> ResponseResult<Json<Value>> {
    let actor = a.auth.authenticate(&h).await?;
    Ok(Json(
        json!({"subject":actor.subject,"permissions":actor.permissions,"roles":p::ROLES}),
    ))
}
async fn list(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Query(q): Query<Page>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::READ).await?;
    let scope = q.scope.into();
    Ok(Json(paginated(
        a.store.list_images(&u.subject, &q.after, scope).await?,
        |i| &i.id,
        |image| image_view(image, &u.subject),
    )))
}
async fn create(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Json(input): Json<CreateImage>,
) -> ResponseResult<(StatusCode, Json<Value>)> {
    let u = actor(&a, &h, p::CREATE).await?;
    Ok((
        StatusCode::CREATED,
        Json(image_view(
            a.store.create_image(&u.subject, key(&h)?, input).await?,
            &u.subject,
        )),
    ))
}
async fn read(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::READ).await?;
    Ok(Json(image_view(
        a.store.image(&u.subject, &id, false).await?,
        &u.subject,
    )))
}
async fn edit(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<CreateImage>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::CONTROL).await?;
    Ok(Json(image_view(
        a.store
            .update_image(&u.subject, &id, revision(&h)?, input)
            .await?,
        &u.subject,
    )))
}
async fn retire(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::DELETE).await?;
    Ok(Json(image_view(
        a.store.retire(&u.subject, &id, revision(&h)?).await?,
        &u.subject,
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVisibility {
    visibility: ImageVisibility,
}
async fn visibility(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<SetVisibility>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::SHARE).await?;
    Ok(Json(image_view(
        a.store
            .set_visibility(&u.subject, &id, revision(&h)?, input.visibility)
            .await?,
        &u.subject,
    )))
}
async fn share(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, subject)): Path<(String, String)>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::SHARE).await?;
    Ok(Json(image_view(
        a.store
            .share(&u.subject, &id, revision(&h)?, &subject, true)
            .await?,
        &u.subject,
    )))
}
async fn unshare(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, subject)): Path<(String, String)>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::SHARE).await?;
    Ok(Json(image_view(
        a.store
            .share(&u.subject, &id, revision(&h)?, &subject, false)
            .await?,
        &u.subject,
    )))
}
async fn build(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<BuildInput>,
) -> ResponseResult<(StatusCode, Json<Value>)> {
    let u = actor(&a, &h, p::BUILD).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(build_view(
            a.store
                .create_build(&u.subject, &id, key(&h)?, input)
                .await?,
        )),
    ))
}
async fn builds(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Page>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::BUILD_READ).await?;
    Ok(Json(paginated(
        a.store
            .image_builds(&u.subject, &id, false, &q.after)
            .await?,
        |b| &b.id,
        build_view,
    )))
}
async fn versions(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Page>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::READ).await?;
    Ok(Json(paginated(
        a.store
            .image_builds(&u.subject, &id, true, &q.after)
            .await?,
        |b| &b.id,
        version_view,
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    version: String,
}
async fn resolve(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Resolve>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::USE).await?;
    Ok(Json(version_view(
        a.store.resolve(&u.subject, &id, &input.version).await?,
    )))
}
async fn version(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path((id, version)): Path<(String, String)>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::READ).await?;
    Ok(Json(version_view(
        a.store.version(&u.subject, &id, &version).await?,
    )))
}
async fn build_read(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::BUILD_READ).await?;
    Ok(Json(build_view(a.store.build(&u.subject, &id).await?)))
}
async fn logs(
    State(a): State<Arc<App>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> ResponseResult<Json<Value>> {
    let u = actor(&a, &h, p::BUILD_READ).await?;
    let build = a.store.build(&u.subject, &id).await?;
    Ok(Json(a.executor.logs(&build).await?))
}
