//! Dedicated path avoids the legacy report-ID wildcard during mixed rollout.
use super::*;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ViewParams {
    window: ViewWindow,
    from: Option<u64>,
    to: Option<u64>,
    step_seconds: Option<u64>,
    filters: Option<String>,
    refresh: bool,
}
pub(super) async fn latest(
    State(service): Service,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Extension(Admin(admin)): Extension<Admin>,
    params: std::result::Result<Query<ViewParams>, QueryRejection>,
) -> Response {
    if let Err(error) = authorize(&caller, admin) {
        return problem(error);
    }
    let Ok(Query(params)) = params else {
        return invalid();
    };
    let filters = match params.filters {
        Some(value) if value.len() <= 4096 => match serde_json::from_str(&value) {
            Ok(filters) => filters,
            Err(_) => return invalid(),
        },
        Some(_) => return invalid(),
        None => Filters::default(),
    };
    let input = ViewInput {
        window: params.window,
        from: params.from,
        to: params.to,
        step_seconds: params.step_seconds,
        filters,
        refresh: params.refresh,
    };
    match bounded(service.latest(&caller, admin, input, now())).await {
        Ok(view) => {
            let status = if view.report.status == ReportStatus::Ready {
                StatusCode::OK
            } else {
                StatusCode::ACCEPTED
            };
            let mut response = (status, Json(wire::value(&view))).into_response();
            if status == StatusCode::ACCEPTED {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, "2".parse().unwrap());
            }
            response
        }
        Err(error) => problem(error),
    }
}
