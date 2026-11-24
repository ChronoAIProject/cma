use super::*;
mod encoding;
#[cfg(test)]
mod tests;

pub(super) fn routes() -> Router<Arc<MetricsService>> {
    Router::new()
        .route("/initial/reports", post(create))
        .route("/initial/view", post(latest))
        .route("/initial/reports/{report}", post(read))
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024))
}

async fn create(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    query: std::result::Result<Query<Empty>, QueryRejection>,
    input: std::result::Result<Json<InitialReportInput>, JsonRejection>,
) -> Response {
    if let Err(error) = authorize(&caller, admin) {
        return problem(error);
    }
    let Ok(Json(input)) = input else {
        return invalid();
    };
    if query.is_err() {
        return invalid();
    }
    encoding::respond(async {
        let value = service.submit_initial(&caller, admin, input, now()).await?;
        Ok((value.report.status == ReportStatus::Pending, value))
    })
    .await
}

async fn latest(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    query: std::result::Result<Query<Empty>, QueryRejection>,
    input: std::result::Result<Json<InitialViewInput>, JsonRejection>,
) -> Response {
    if let Err(error) = authorize(&caller, admin) {
        return problem(error);
    }
    let Ok(Json(input)) = input else {
        return invalid();
    };
    if query.is_err() {
        return invalid();
    }
    encoding::respond(async {
        let value = service.latest_initial(&caller, admin, input, now()).await?;
        Ok((value.view.report.status == ReportStatus::Pending, value))
    })
    .await
}

async fn read(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    Path(id): Path<String>,
    query: std::result::Result<Query<Empty>, QueryRejection>,
    input: std::result::Result<Json<InitialProjection>, JsonRejection>,
) -> Response {
    if let Err(error) = authorize(&caller, admin) {
        return problem(error);
    }
    let Ok(Json(projection)) = input else {
        return invalid();
    };
    if query.is_err() {
        return invalid();
    }
    encoding::respond(async {
        let value = service
            .initial_report(&caller, admin, &id, projection, now())
            .await?;
        Ok((value.report.status == ReportStatus::Pending, value))
    })
    .await
}
