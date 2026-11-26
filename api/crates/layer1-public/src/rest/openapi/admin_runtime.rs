//! Operator runtime update and pool management contracts.
use crate::rest::admin::{runtime_migration, runtime_update, warm_pools};
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        runtime_update::update,
        runtime_migration::migrate,
        runtime_migration::observe,
        warm_pools::list,
        warm_pools::configure,
        warm_pools::resize,
    ),
    components(schemas(
        warm_pools::WarmPoolList,
        warm_pools::WarmPoolView,
        warm_pools::ConfigureWarmPoolBody,
        warm_pools::ResizeWarmPoolBody,
    ))
)]
pub(super) struct AdminRuntimeApi;
