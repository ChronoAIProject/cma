use super::*;
use axum::{
    extract::{FromRequestParts, RawPathParams, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer1_public::{rest::HttpProblem, AuthenticatedSubject};

/// Private in-process proof for the downstream generic rate limiter. A client
/// header cannot construct it or bypass this independently charged policy.
#[derive(Clone)]
pub(crate) struct Admitted;

pub(crate) async fn enforce(
    State(policy): State<Arc<dyn LifecycleAdmissionPolicy>>,
    request: Request,
    next: Next,
) -> Response {
    if !policy.enabled() || request.extensions().get::<Admitted>().is_some() {
        return next.run(request).await;
    }
    let Some((class, _)) = classify(&request) else {
        return next.run(request).await;
    };
    // Axum's decoded route parameter keeps encoded aliases in one target slot.
    let (mut parts, body) = request.into_parts();
    let params = match RawPathParams::from_request_parts(&mut parts, &()).await {
        Ok(params) => params,
        Err(error) => return error.into_response(),
    };
    let target = params
        .iter()
        .find(|(name, _)| *name == "id")
        .map(|(_, value)| value.to_owned())
        .expect("registered Agent route has id");
    let mut request = Request::from_parts(parts, body);
    let Some(caller) = request.extensions().get::<AuthenticatedSubject>() else {
        return layer1_public::auth::NotAuthenticated.into_response();
    };
    let permit = match policy
        .admit(caller.subject().as_str(), &target, class)
        .await
    {
        Ok(permit) => permit,
        Err(Refusal::Caller(retry_after_seconds)) => {
            metrics::counter!("cma_agent_control_refusals_total", "class" => class.label(), "reason" => "caller").increment(1);
            return HttpProblem::RateLimited {
                retry_after_seconds,
            }
            .into_response();
        }
        Err(Refusal::Unavailable | Refusal::Capacity) => {
            metrics::counter!("cma_agent_control_refusals_total", "class" => class.label(), "reason" => "unavailable").increment(1);
            return HttpProblem::RequestBudgetUnavailable.into_response();
        }
    };
    request.extensions_mut().insert(Admitted);
    // Request cancellation must not make capacity reusable while the execution
    // or its driver work is still being joined. The bounded stage owns custody.
    let context = layer2_core::diagnostic_context::DiagnosticContext::current();
    let execution = tokio::spawn(context.scope(async move {
        let _permit = permit;
        let _priority = crate::background::shared().foreground();
        next.run(request).await
    }));
    match execution.await {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(
                class = class.label(),
                "Lifecycle request task ended without a response"
            );
            HttpProblem::RequestBudgetUnavailable.into_response()
        }
    }
}
