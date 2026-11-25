use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::{Sandbox, SandboxStoreError};
use layer3_adapter::RuntimeProvisionRequest;

use super::{
    support::{adapter_failure, WorkerFailure},
    SandboxWorker,
};

impl SandboxWorker {
    pub(super) async fn prepare_sandbox_home(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<Option<WorkerFailure>, SandboxStoreError> {
        let canary_image = self
            .dependencies
            .store
            .canary_runtime_image(sandbox)
            .await?;
        let native_pool = sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.native_pool.as_ref());
        if canary_image.is_none()
            && native_pool.is_some_and(|pool| pool.request.pool.spec.cold_canary)
        {
            return Err(SandboxStoreError::Corrupt(
                "canary Sandbox is missing its runtime authority".into(),
            ));
        }
        if let Some(image) = &canary_image {
            let matches_home = match sandbox.lifecycle.home.as_ref() {
                Some(home) => native_pool.is_some_and(|pool| {
                    pool.request.pool.spec.cold_canary
                        && &pool.request.pool.spec.image == image
                        && home.request.policy.is_native_vct()
                        && home.request.policy.mounts_full_home()
                }),
                None => {
                    sandbox.runtime.is_none()
                        && self.config.home_policy.as_ref().is_some_and(|policy| {
                            policy.is_native_vct() && policy.mounts_full_home()
                        })
                }
            };
            if !matches_home {
                return Err(SandboxStoreError::Corrupt(
                    "canary Sandbox requires its exact native full-home allocation".into(),
                ));
            }
        }
        // A Ready warm reservation already verified this exact native home.
        // Keep the owner-side runtime inspection below as the live fence, but
        // avoid repeating the storage read before it. If the runtime vanished,
        // retain the marker so replacement provisioning re-enters this path.
        if sandbox.runtime.is_some()
            && sandbox
                .lifecycle
                .home
                .as_ref()
                .is_some_and(|home| home.native_pool.is_some() && home.attachment().is_some())
            && sandbox.work.prepared_installation_digest.is_some()
        {
            return Ok(None);
        }
        // Existing ephemeral bindings retain their exact request identity. A
        // later revival can allocate a home before restoring their checkpoint.
        if sandbox.runtime.is_some() && sandbox.lifecycle.home.is_none() {
            return Ok(None);
        }
        let mut release_retries = 0;
        loop {
            let mut timing = super::provision_timing::ProvisionTiming::new();
            let phase = home_phase(sandbox.lifecycle.home.as_deref());
            let mut release_fence = None;
            let next = match crate::sandbox::home::next_home(
                self.dependencies.adapter.as_ref(),
                &sandbox.id,
                sandbox.lifecycle.home.as_deref(),
                self.config.home_policy.as_ref(),
            )
            .await
            {
                Ok(Some(home)) if sandbox.lifecycle.home.is_none() => {
                    // A canary's pin comes from its verified receipt. An
                    // ordinary lifecycle may already own a released image.
                    let (admitted, released) = if canary_image.is_some() {
                        (None, None)
                    } else if let Some(admitted) =
                        super::initial_runtime::managed_desktop_creation_release(sandbox)
                    {
                        (Some(admitted), None)
                    } else {
                        let admitted = self
                            .dependencies
                            .store
                            .admitted_runtime_release(sandbox)
                            .await?;
                        let released = if admitted.is_none() {
                            self.config.release()
                        } else {
                            None
                        };
                        (admitted, released)
                    };
                    let mut request = RuntimeProvisionRequest::new(
                        sandbox.lifecycle.runtime_generation_high_water.max(1),
                        crate::sandbox::runtime_mapping::sandbox_spec(
                            sandbox,
                            &self.config.deployment_id,
                        )
                        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
                    );
                    request.native_canary_image = canary_image.clone();
                    request.released = admitted.or_else(|| released.clone());
                    let reserved = crate::sandbox::home_native::reserve(
                        self.dependencies.adapter.as_ref(),
                        &request,
                        home,
                    )
                    .await
                    .and_then(|home| {
                        match (&self.config.released, &released) {
                            (Some(source), Some(released)) => source
                                .require_current(released)
                                .map(|()| home)
                                .map_err(|stale| {
                                    tracing::info!(
                                        sandbox_id = %sandbox.id,
                                        revision = released.revision,
                                        "home reservation retried after the released runtime changed"
                                    );
                                    layer3_adapter::AdapterError::Unavailable(stale.to_string())
                                }),
                            _ => Ok(home),
                        }
                    });
                    match reserved {
                        Ok(home) => {
                            if request.spec.runtime_image.is_none() {
                                release_fence = released;
                            }
                            home
                        }
                        Err(error) => return Ok(Some(adapter_failure(error))),
                    }
                }
                Ok(Some(home)) => home,
                Ok(None) => {
                    timing.completed(phase);
                    return Ok(None);
                }
                Err(error) => return Ok(Some(adapter_failure(error))),
            };
            timing.completed(phase);
            if !self
                .save_home(sandbox, next, now, release_fence.as_ref())
                .await?
            {
                release_retries += 1;
                if release_retries == 4 {
                    return Ok(Some(adapter_failure(
                        layer3_adapter::AdapterError::Unavailable(
                            layer3_adapter::StaleReleasedRuntime.to_string(),
                        ),
                    )));
                }
                continue;
            }
            timing.completed("home_persist");
        }
    }

    pub(super) async fn cleanup_native_home(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<Option<WorkerFailure>, SandboxStoreError> {
        loop {
            if let Some(failure) = self.prepare_retirement_cleanup(sandbox, now).await? {
                return Ok(Some(failure));
            }
            let Some(home) = sandbox.lifecycle.home.as_deref() else {
                return Ok(None);
            };
            let next = match crate::sandbox::home_native::next_cleanup(
                self.dependencies.adapter.as_ref(),
                home,
            )
            .await
            {
                Ok(Some(next)) => next,
                Ok(None) => return Ok(None),
                Err(error) => return Ok(Some(adapter_failure(error))),
            };
            self.save_home(sandbox, next, now, None).await?;
        }
    }

    async fn save_home(
        &self,
        sandbox: &mut Sandbox,
        home: layer2_core::sandbox::SandboxHome,
        now: SystemTime,
        release: Option<&layer3_adapter::ReleasedRuntime>,
    ) -> Result<bool, SandboxStoreError> {
        for _ in 0..4 {
            let expected = sandbox.revision;
            let mut updated = sandbox.clone();
            updated.lifecycle.home = Some(Box::new(home.clone()));
            updated.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            updated.updated_at = updated.updated_at.max(rfc3339_at(now));
            let saved = match release {
                Some(release) => {
                    self.dependencies
                        .store
                        .save_sandbox_against_release(
                            expected,
                            updated.clone(),
                            release,
                            layer2_core::released_runtime::ReleasedLane::Agent,
                        )
                        .await
                }
                None => {
                    self.dependencies
                        .store
                        .save_sandbox(expected, updated.clone())
                        .await
                }
            };
            match saved {
                Ok(()) => {
                    *sandbox = updated;
                    return Ok(true);
                }
                Err(SandboxStoreError::Conflict) => {
                    let current = self
                        .dependencies
                        .store
                        .get_sandbox(&sandbox.id)
                        .await?
                        .ok_or(SandboxStoreError::Conflict)?;
                    // Early Agent admission touches parent activity while this
                    // exact worker is creating its durable native Claim. Keep
                    // that metadata, but never rebase over a different lease,
                    // home, runtime, credential, phase, or deletion intent.
                    let mut previous = current.clone();
                    previous.revision = sandbox.revision;
                    previous.updated_at = sandbox.updated_at.clone();
                    previous.last_activity_at = sandbox.last_activity_at.clone();
                    if !layer2_core::sandbox::store::sandbox_content_matches(
                        Some(&previous),
                        sandbox,
                    ) {
                        return Err(SandboxStoreError::Conflict);
                    }
                    *sandbox = current;
                    if let (Some(source), Some(release)) = (&self.config.released, release) {
                        // The transaction can see a promotion before this replica's
                        // cache. Replan the uncommitted home before creating a Claim.
                        source.refresh_after_stale().await;
                        let refreshed = source.snapshot();
                        if refreshed.agent != release.agent
                            || refreshed.lane_revisions.map(|value| value.agent)
                                != release.lane_revisions.map(|value| value.agent)
                        {
                            tracing::info!(
                                sandbox_id = %sandbox.id,
                                captured_revision = release.revision,
                                current_revision = refreshed.revision,
                                "retrying home reservation after durable release changed"
                            );
                            return Ok(false);
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

fn home_phase(home: Option<&layer2_core::sandbox::SandboxHome>) -> &'static str {
    let Some(home) = home else {
        return "home_reserve";
    };
    if home.receipt.is_some() {
        return "home_verify";
    }
    match home.native_pool.as_ref().map(|native| &native.claim) {
        Some(None) => "home_claim_create",
        Some(Some(claim)) if claim.sandbox_name.is_none() => "home_claim_assignment",
        Some(Some(_)) => "home_adoption",
        None => "home_legacy_prepare",
    }
}
