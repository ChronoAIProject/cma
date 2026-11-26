use super::*;
use crate::rest::{instances::confirmed_digest, instances_openapi_shapes::CreateSandboxRequest};
use axum::Extension;
use layer2_core::{
    profiles::SandboxProfileRef,
    sandbox::{CreateSandbox, SandboxOrigin},
    SandboxProfileId,
};

#[utoipa::path(
    post,
    path = "/api/v1/sandboxes",
    tag = "sandboxes",
    params(("Idempotency-Key" = String, Header)),
    request_body = CreateSandboxRequest,
    responses(
        (status = 201, body = SandboxView),
        (status = 409, body = crate::rest::ProblemDocument),
        (status = 429, body = crate::rest::ProblemDocument)
    )
)]
pub(in crate::rest) async fn create(
    State(state): State<HierarchyState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    creation: Option<Extension<std::sync::Arc<dyn crate::rest::creation_outcomes::Backend>>>,
    key: IdempotencyKey,
    body: Result<RequestBody<CreateSandboxRequest>, BodyProblem>,
) -> Response {
    use crate::rest::creation_outcomes::{
        routes::{begin_submission, problem},
        Operation, RefusalCode,
    };
    use layer2_core::sandbox::SandboxServiceError;
    if !caller.authorization.allows(Capability::CreateSandbox) {
        return HierarchyProblem::Forbidden.into_response();
    }
    let backend = creation.map(|Extension(backend)| backend);
    let fence = if key
        .value()
        .is_some_and(crate::rest::creation_outcomes::is_covered_key)
    {
        match begin_submission(
            backend.as_deref(),
            &caller,
            Operation::Sandbox,
            key.value(),
            body.as_ref().ok().and_then(RequestBody::creation_digest),
        )
        .await
        {
            Ok(Some(submission)) => submission.fence,
            Ok(None) => return problem(StatusCode::CONFLICT),
            Err(response) => return response,
        }
    } else {
        None
    };
    let command_state = state.clone();
    let command_caller = caller.clone();
    execute_command(
        &state,
        &caller,
        Capability::CreateSandbox,
        key,
        "sandbox:create".into(),
        body,
        move |body| async move {
            let confirmed_digest = confirmed_digest(body.trust_confirmed, body.commands_digest)?;
            let result = command_state
                .sandboxes
                .create_with_fence(
                    &command_caller,
                    CreateSandbox {
                        profile: SandboxProfileRef::new(
                            SandboxProfileId(body.sandbox_profile.id),
                            body.sandbox_profile.revision,
                        ),
                        name: body.name,
                        confirmed_digest,
                        origin: SandboxOrigin::Manual,
                    },
                    fence.clone(),
                )
                .await;
            if let (Some(fence), Some(backend), Err(error)) = (&fence, &backend, &result) {
                let code = match error {
                    SandboxServiceError::SandboxLimitReached { .. } => {
                        Some(RefusalCode::SandboxLimitReached)
                    }
                    SandboxServiceError::TrustConfirmationRequired { .. } => {
                        Some(RefusalCode::TrustConfirmationRequired)
                    }
                    _ => None,
                };
                if let Some(code) = code {
                    if !matches!(
                        backend
                            .refuse_direct(&command_caller.subject, fence, code)
                            .await,
                        Ok(true)
                    ) {
                        return Err(HierarchyProblem::Unavailable);
                    }
                }
            }
            let sandbox = result.map_err(HierarchyProblem::sandbox)?;
            let mut view = SandboxView::from(&sandbox);
            view.color = crate::rest::sandbox_colors::colors(
                &command_state,
                &command_caller.subject,
                &[&sandbox],
            )
            .await
            .remove(&view.id);
            Ok((StatusCode::CREATED, Json(view)).into_response())
        },
    )
    .await
}
