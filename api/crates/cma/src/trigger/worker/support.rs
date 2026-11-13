use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::profiles::content_digest;
use layer2_core::trigger_domain::{
    Launch, LaunchCommit, LaunchEffectState, LaunchEventKind, LaunchState, ResolvedResource,
    ResolvedTriggerProfiles, TriggerRevision, TriggerTarget, MAX_LAUNCH_RETRY_DELAY_SECONDS,
};
use layer2_core::{rfc3339_at, AgentId, Authorization, Caller, Capability, SandboxId, WorkspaceId};

use super::driver::LaunchResourceError;
use super::{LaunchWorker, LaunchWorkerError};

pub(super) struct FrozenLaunchContext {
    pub(super) revision: TriggerRevision,
    pub(super) profiles: ResolvedTriggerProfiles,
}

#[derive(Default)]
pub(super) struct FrozenContextCache {
    value: Option<(FrozenContextKey, Arc<FrozenLaunchContext>)>,
}

#[derive(PartialEq, Eq)]
struct FrozenContextKey {
    launch_id: layer2_core::LaunchId,
    revision_digest: String,
    existing: Option<(SandboxId, WorkspaceId, AgentId)>,
}

impl FrozenContextKey {
    fn for_launch(launch: &Launch) -> Self {
        let existing = match (
            &launch.resolution.sandbox,
            &launch.resolution.workspace,
            &launch.resolution.agent,
            launch.first_message,
        ) {
            (
                ResolvedResource::Reused(sandbox),
                ResolvedResource::Reused(workspace),
                ResolvedResource::Reused(agent),
                layer2_core::sandbox::FirstMessageDelivery::Completed,
            ) => Some((sandbox.clone(), workspace.clone(), agent.clone())),
            _ => None,
        };
        Self {
            launch_id: launch.id.clone(),
            revision_digest: launch.revision_digest.clone(),
            existing,
        }
    }
}

impl FrozenLaunchContext {
    fn validate(&self, launch: &Launch, target: &TriggerTarget) -> Result<(), LaunchWorkerError> {
        self.revision
            .validate()
            .map_err(|error| corrupt(error.to_string()))?;
        let profiles = &self.profiles;
        let exact = self.revision.digest == launch.revision_digest
            && profiles.sandbox.profile_id == target.sandbox_profile.id
            && profiles.sandbox.revision == target.sandbox_profile.revision
            && profiles.workspace.profile_id == target.workspace_profile.id
            && profiles.workspace.revision == target.workspace_profile.revision
            && profiles.agent.profile_id == target.agent_profile.id
            && profiles.agent.revision == target.agent_profile.revision
            && profiles.sandbox.content.workspace_profiles.len()
                == profiles.sandbox_workspaces.len()
            && profiles
                .sandbox
                .content
                .workspace_profiles
                .iter()
                .zip(&profiles.sandbox_workspaces)
                .all(|(reference, revision)| {
                    reference.id == revision.profile_id && reference.revision == revision.revision
                });
        let digests = content_digest(&profiles.sandbox.content).ok().as_ref()
            == Some(&profiles.sandbox.digest)
            && content_digest(&profiles.workspace.content).ok().as_ref()
                == Some(&profiles.workspace.digest)
            && content_digest(&profiles.agent.content).ok().as_ref()
                == Some(&profiles.agent.digest)
            && profiles.sandbox_workspaces.iter().all(|revision| {
                content_digest(&revision.content).ok().as_ref() == Some(&revision.digest)
            });
        if exact && digests {
            Ok(())
        } else {
            Err(corrupt("frozen Trigger inputs do not match the Launch"))
        }
    }
}

impl LaunchWorker {
    pub(super) async fn cached_frozen_context(
        &self,
        launch: Launch,
        now: SystemTime,
        cache: &mut FrozenContextCache,
    ) -> Result<Option<(Launch, Arc<FrozenLaunchContext>)>, LaunchWorkerError> {
        let key = FrozenContextKey::for_launch(&launch);
        if let Some((cached_key, context)) = &cache.value {
            if *cached_key == key {
                // Exact inputs remain cached within this scheduled run. New
                // catalogue effects serialize at admission; a terminal refusal
                // rechecks publisher erasure before it is settled.
                return Ok(Some((launch, Arc::clone(context))));
            }
        }
        // Cache only immutable, validated inputs for this scheduled run. Control,
        // cancellation and resource readiness are checked anew at every durable claim.
        // Switching to an existing conversation invalidates the cache to retain its pins.
        cache.value = None;
        let Some((launch, context)) = self.load_frozen_context(launch, now).await? else {
            return Ok(None);
        };
        let context = Arc::new(context);
        cache.value = Some((key, Arc::clone(&context)));
        Ok(Some((launch, context)))
    }

    pub(super) async fn load_frozen_context(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<Option<(Launch, FrozenLaunchContext)>, LaunchWorkerError> {
        let revision = match self
            .resources
            .trigger_revision(&launch.trigger_id, &launch.revision_digest)
            .await
        {
            Ok(Some(revision)) => revision,
            Ok(None) => {
                let failure = self.missing_trigger_revision(&launch).await?;
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(None);
            }
            Err(failure) => {
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(None);
            }
        };
        let retained = if revision.publisher_deleted {
            self.retained_agent_target(&launch).await.map(Some)
        } else {
            self.existing_conversation_target(&launch).await
        };
        let catalogue_target = matches!(&retained, Ok(None));
        let target = match retained {
            Ok(Some(target)) => target,
            Ok(None) => revision.content.target.clone(),
            Err(failure) => {
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(None);
            }
        };
        let profiles = match self.resources.frozen_profiles(&target).await {
            Ok(profiles) => profiles,
            Err(mut failure) => {
                if catalogue_target
                    && !revision.publisher_deleted
                    && failure.code == "frozen_profile_missing"
                {
                    // Erasure can sanitize the publisher and delete an unreferenced
                    // Profile after this claim read the ordinary Trigger revision.
                    failure = self.recheck_publisher_erasure(&launch, failure).await?;
                }
                self.settle_resource_failure(launch, failure, now).await?;
                return Ok(None);
            }
        };
        let context = FrozenLaunchContext { revision, profiles };
        if let Err(error) = context.validate(&launch, &target) {
            self.settle_resource_failure(
                launch,
                LaunchResourceError::terminal(
                    "frozen_launch_invalid",
                    "Frozen launch inputs are not usable",
                ),
                now,
            )
            .await?;
            tracing::error!(error = %error, "frozen launch validation failed");
            return Ok(None);
        }
        Ok(Some((launch, context)))
    }

    pub(super) async fn commit(
        &self,
        expected_revision: u64,
        launch: Launch,
        event_at: String,
        events: Vec<LaunchEventKind>,
    ) -> Result<Launch, LaunchWorkerError> {
        self.launches
            .commit_launch(LaunchCommit {
                expected_revision,
                launch,
                event_at,
                events,
            })
            .await
            .map_err(Into::into)
    }

    pub(super) async fn defer(
        &self,
        mut launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let delay = if launch.state == LaunchState::Ready {
            self.config.poll_delay.max(Duration::from_secs(30))
        } else {
            self.config.poll_delay
        };
        let events = launch
            .defer_worker_launch(
                &self.config.holder,
                fence,
                rfc3339_at(now + delay),
                at.clone(),
            )
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    pub(super) async fn settle_resource_failure(
        &self,
        mut launch: Launch,
        failure: LaunchResourceError,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if launch.state == LaunchState::Ready {
            tracing::error!(
                launch_id = %launch.id,
                code = failure.code,
                "ready Launch dependency could not be observed"
            );
            return self.defer(launch, now).await;
        }
        if failure.retryable
            && launch
                .work
                .effect
                .as_ref()
                .is_some_and(|effect| effect.state == LaunchEffectState::Applied)
        {
            return self.defer(launch, now).await;
        }
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = if failure.retryable {
            launch.retry_worker_launch(
                &self.config.holder,
                fence,
                failure.code.into(),
                failure.safe_detail.into(),
                self.retry_at(&launch, now),
                at.clone(),
            )
        } else {
            launch.fail_worker_launch(
                &self.config.holder,
                fence,
                failure.code.into(),
                failure.safe_detail.into(),
                at.clone(),
            )
        }
        .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    pub(super) fn caller(&self, launch: &Launch) -> (Caller, Capability) {
        let authority = match launch.authority {
            layer2_core::trigger_domain::LaunchAuthorityClass::Visitor => {
                Capability::LaunchTriggers
            }
            layer2_core::trigger_domain::LaunchAuthorityClass::SandboxCreator => {
                Capability::CreateSandbox
            }
        };
        (
            Caller::new(
                launch.owner.clone(),
                Authorization::new(std::iter::once(authority)),
            ),
            authority,
        )
    }

    pub(super) fn fence(&self, launch: &Launch) -> Result<u64, LaunchWorkerError> {
        launch
            .work
            .lease
            .as_ref()
            .filter(|lease| lease.holder == self.config.holder)
            .map(|lease| lease.fence)
            .ok_or_else(|| corrupt("claimed Launch has no matching worker fence"))
    }

    fn retry_at(&self, launch: &Launch, now: SystemTime) -> String {
        let exponent = launch.work.attempts.min(30);
        let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
        let seconds = self
            .config
            .retry_delay
            .as_secs()
            .saturating_mul(multiplier)
            .min(MAX_LAUNCH_RETRY_DELAY_SECONDS);
        rfc3339_at(now + Duration::from_secs(seconds))
    }
}

pub(super) fn validate_sandbox_id(value: &str) -> Result<SandboxId, LaunchResourceError> {
    valid_resource_id(value, "sbx_")
        .then(|| SandboxId(value.into()))
        .ok_or_else(invalid_effect_result)
}

pub(super) fn validate_workspace_id(value: &str) -> Result<WorkspaceId, LaunchResourceError> {
    valid_resource_id(value, "wks_")
        .then(|| WorkspaceId(value.into()))
        .ok_or_else(invalid_effect_result)
}

pub(super) fn validate_agent_id(value: &str) -> Result<AgentId, LaunchResourceError> {
    valid_resource_id(value, "agt_")
        .then(|| AgentId(value.into()))
        .ok_or_else(invalid_effect_result)
}

pub(super) fn valid_resource_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
}

pub(super) fn valid_origin_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

pub(super) fn domain_error(error: layer2_core::trigger_domain::TriggerError) -> LaunchWorkerError {
    corrupt(format!("{} at {}", error.code, error.field))
}

pub(super) fn corrupt(detail: impl Into<String>) -> LaunchWorkerError {
    LaunchWorkerError::Corrupt(detail.into())
}

pub(super) fn missing_resource(resource: &'static str) -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_resource_missing",
        match resource {
            "sandbox" => "The Sandbox selected for this launch is unavailable",
            "workspace" => "The Workspace selected for this launch is unavailable",
            "agent" => "The Agent selected for this launch is unavailable",
            _ => "A resource selected for this launch is unavailable",
        },
    )
}

fn invalid_effect_result() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_effect_result_invalid",
        "A launch dependency returned invalid resource evidence",
    )
}
