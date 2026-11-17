use super::{runtime_failure, Supervisor};
use axum::{extract::State, response::Response, Json};
use layer3_adapter::desktop_ownership::wire::{DesktopChallengeRequest, DesktopConfirmation};
use layer3_adapter::DesktopOwnership;
use std::sync::Arc;

fn capacity(supervisor: &Supervisor) -> crate::failure::Result<tokio::sync::OwnedSemaphorePermit> {
    supervisor
        .desktop_ownership
        .controls
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            crate::failure::RuntimeError::unavailable(
                "desktop_control_busy",
                "desktop control capacity is exhausted",
            )
        })
}

pub(super) async fn challenge(
    State(supervisor): State<Arc<Supervisor>>,
    Json(request): Json<DesktopChallengeRequest>,
) -> Result<Json<layer3_adapter::DesktopOwnershipChallenge>, Response> {
    let _permit = capacity(&supervisor).map_err(runtime_failure)?;
    supervisor
        .desktop_ownership
        .challenge(request)
        .map(Json)
        .map_err(runtime_failure)
}

pub(super) async fn confirm(
    State(supervisor): State<Arc<Supervisor>>,
    Json(request): Json<DesktopConfirmation>,
) -> Result<Json<layer3_adapter::desktop_ownership::wire::DesktopAcknowledgement>, Response> {
    let _permit = capacity(&supervisor).map_err(runtime_failure)?;
    supervisor
        .desktop_ownership
        .confirm(request)
        .await
        .map(Json)
        .map_err(runtime_failure)
}

pub(super) async fn revoke(
    State(supervisor): State<Arc<Supervisor>>,
    Json(request): Json<DesktopOwnership>,
) -> Result<Json<layer3_adapter::desktop_ownership::wire::DesktopAcknowledgement>, Response> {
    let _permit = capacity(&supervisor).map_err(runtime_failure)?;
    supervisor
        .desktop_ownership
        .revoke(request)
        .await
        .map(Json)
        .map_err(runtime_failure)
}
