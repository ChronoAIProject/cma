use crate::{now_rfc3339, Caller, LaunchId};

use super::launch_service_support::{
    connection_error, store_error, trigger_error, validate_requirement_identity,
    validate_start_request,
};
use super::launch_trust::LaunchTrustManifest;
use super::{
    Launch, LaunchCommit, LaunchConfirmation, LaunchConsentSurface, LaunchEvent, LaunchEventKind,
    LaunchPage, LaunchPageRequest, LaunchService, LaunchServiceError, LaunchState,
    RequirementsSnapshot, ReviewRequirements, StartLaunch, TriggerRevision,
    DEFAULT_LAUNCH_EVENT_LIMIT,
};

impl LaunchService {
    pub async fn start(
        &self,
        caller: &Caller,
        id: &LaunchId,
        request: StartLaunch,
    ) -> Result<Launch, LaunchServiceError> {
        validate_start_request(&request)?;
        if !self.policy.current_launch_policy().await?.enabled {
            return Err(LaunchServiceError::Disabled);
        }
        let mut launch = self.owned(id, &caller.subject).await?;
        if launch.state != LaunchState::Draft {
            return Err(LaunchServiceError::NotStartable);
        }
        let revision = self.launch_revision(&launch).await?;
        let consent_recorded = !launch.confirmations.is_empty();
        if consent_recorded && revision.content.consent.always_review {
            return Err(LaunchServiceError::Corrupt(
                "Publisher-required review was not completed".into(),
            ));
        }
        if consent_recorded && !request.acknowledgements.is_empty() {
            return Err(LaunchServiceError::InvalidRequest(
                "Start cannot replace the recorded landing confirmation".into(),
            ));
        }
        let unconfirmed_digest = if consent_recorded {
            None
        } else {
            launch.trust.confirmation_digest.clone()
        };
        let profiles = self
            .profile_reader
            .read_trigger_profiles(&revision.content.target)
            .await
            .map_err(|error| LaunchServiceError::Unavailable(error.to_string()))?;
        if launch.resolution.sandbox.is_pending() {
            self.require_sandbox_credentials(&caller.subject).await?;
        }
        let requirements = RequirementsSnapshot {
            services: self
                .connections
                .requirements(&caller.subject, &profiles.sandbox.content.nyxid_services)
                .await
                .map_err(connection_error)?,
            checked_at: now_rfc3339(),
        };
        requirements
            .validate()
            .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
        validate_requirement_identity(&profiles.sandbox.content.nyxid_services, &requirements)?;
        let changed_events = changed_requirement_events(&launch.requirements, &requirements);
        let missing = requirements
            .services
            .iter()
            .filter(|requirement| requirement.needs_connection())
            .map(|requirement| requirement.service.clone())
            .collect::<Vec<_>>();
        let refreshed_review = ReviewRequirements::build(
            &requirements,
            unconfirmed_digest.clone(),
            revision.content.consent.always_review,
            false,
        )
        .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
        if !missing.is_empty() {
            self.commit_review_refresh(launch, requirements, refreshed_review, changed_events)
                .await?;
            return Err(LaunchServiceError::RequirementsNotMet { services: missing });
        }
        if launch.review.trust_digest() != unconfirmed_digest.as_deref() {
            return Err(LaunchServiceError::Corrupt(
                "Launch review does not match its trust manifest".into(),
            ));
        }
        if launch.review.trust_digest() != request.trust_confirmed_digest.as_deref() {
            self.commit_review_refresh(launch, requirements, refreshed_review, changed_events)
                .await?;
            return Err(LaunchServiceError::TrustConfirmationMismatch);
        }
        let trust = LaunchTrustManifest::resolve_version(
            launch.trust.version,
            self.sandbox_profiles.as_ref(),
            self.workspace_profiles.as_ref(),
            &profiles,
            &launch.resolution,
            &caller.subject,
        )
        .await?;
        if !launch.trust.matches_frozen(&trust) {
            return Err(LaunchServiceError::Corrupt(
                "Launch trust manifest no longer matches its frozen profiles".into(),
            ));
        }
        let expected = launch.revision;
        let mut events = changed_events;
        launch.requirements = requirements;
        launch.review = ReviewRequirements {
            needs: Vec::new(),
            can_start_now: true,
        };
        let now = now_rfc3339();
        events.extend(
            launch
                .sync_requirement_step(&now)
                .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?,
        );
        let started = if consent_recorded {
            launch.start_confirmed(now.clone())
        } else {
            let confirmation = LaunchConfirmation {
                surface: LaunchConsentSurface::Review,
                trust_confirmed_digest: request.trust_confirmed_digest,
                acknowledgements: request.acknowledgements,
                confirmed_at: now.clone(),
            };
            events.push(LaunchEventKind::ConfirmationRecorded);
            if matches!(launch.resolution.agent, super::ResolvedResource::Reused(_)) {
                launch.start_existing(confirmation)
            } else {
                launch.start(confirmation)
            }
        };
        started.map_err(|_| LaunchServiceError::NotStartable)?;
        events.push(LaunchEventKind::StateChanged {
            from: LaunchState::Draft,
            to: LaunchState::Resolving,
        });
        self.launches
            .commit_launch(LaunchCommit {
                expected_revision: expected,
                launch,
                event_at: now,
                events,
            })
            .await
            .map_err(store_error)
    }

    pub async fn cancel(
        &self,
        caller: &Caller,
        id: &LaunchId,
    ) -> Result<Launch, LaunchServiceError> {
        let mut launch = self.owned(id, &caller.subject).await?;
        let from = launch.state;
        let expected = launch.revision;
        let now = now_rfc3339();
        launch
            .cancel(now.clone())
            .map_err(|_| LaunchServiceError::NotStartable)?;
        self.launches
            .commit_launch(LaunchCommit {
                expected_revision: expected,
                launch,
                event_at: now,
                events: vec![
                    LaunchEventKind::StateChanged {
                        from,
                        to: LaunchState::Cancelled,
                    },
                    LaunchEventKind::Cancelled,
                ],
            })
            .await
            .map_err(store_error)
    }

    pub async fn read(&self, caller: &Caller, id: &LaunchId) -> Result<Launch, LaunchServiceError> {
        self.owned(id, &caller.subject).await
    }

    pub async fn list(
        &self,
        caller: &Caller,
        page: &LaunchPageRequest,
    ) -> Result<LaunchPage, LaunchServiceError> {
        page.validate().map_err(store_error)?;
        self.launches
            .list_launches(&caller.subject, page)
            .await
            .map_err(store_error)
    }

    pub async fn requirements(
        &self,
        caller: &Caller,
        id: &LaunchId,
    ) -> Result<RequirementsSnapshot, LaunchServiceError> {
        Ok(self.read(caller, id).await?.requirements)
    }

    pub async fn events(
        &self,
        caller: &Caller,
        id: &LaunchId,
        after: u64,
        limit: Option<u16>,
    ) -> Result<Vec<LaunchEvent>, LaunchServiceError> {
        self.launches
            .launch_events(
                id,
                &caller.subject,
                after,
                limit.unwrap_or(DEFAULT_LAUNCH_EVENT_LIMIT),
            )
            .await
            .map_err(store_error)
    }

    async fn launch_revision(
        &self,
        launch: &Launch,
    ) -> Result<TriggerRevision, LaunchServiceError> {
        let public = self
            .definitions
            .trigger_by_public_id(&launch.public_id)
            .await
            .map_err(trigger_error)?
            .ok_or_else(|| {
                LaunchServiceError::Corrupt("Launch public mapping is missing".into())
            })?;
        if public.public_id != launch.public_id || public.trigger_id != launch.trigger_id {
            return Err(LaunchServiceError::Corrupt(
                "Launch public mapping does not match".into(),
            ));
        }
        let revision = self
            .definitions
            .trigger_revision(&launch.trigger_id, &launch.revision_digest)
            .await
            .map_err(trigger_error)?
            .ok_or_else(|| LaunchServiceError::Corrupt("Launch revision is missing".into()))?;
        revision
            .validate()
            .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
        if revision.digest != launch.revision_digest {
            return Err(LaunchServiceError::Corrupt(
                "Launch revision digest does not match".into(),
            ));
        }
        Ok(revision)
    }

    async fn commit_review_refresh(
        &self,
        mut launch: Launch,
        requirements: RequirementsSnapshot,
        review: ReviewRequirements,
        mut events: Vec<LaunchEventKind>,
    ) -> Result<Launch, LaunchServiceError> {
        let expected_revision = launch.revision;
        let now = now_rfc3339();
        let progress = launch
            .refresh_review(requirements, review, now.clone())
            .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
        events.extend(progress);
        self.launches
            .commit_launch(LaunchCommit {
                expected_revision,
                launch,
                event_at: now,
                events,
            })
            .await
            .map_err(store_error)
    }
}

fn changed_requirement_events(
    previous: &RequirementsSnapshot,
    refreshed: &RequirementsSnapshot,
) -> Vec<LaunchEventKind> {
    refreshed
        .services
        .iter()
        .filter(|refreshed| {
            previous
                .services
                .iter()
                .find(|previous| previous.service == refreshed.service)
                .is_none_or(|previous| previous.state != refreshed.state)
        })
        .map(|refreshed| LaunchEventKind::RequirementChanged {
            service: refreshed.service.clone(),
            state: refreshed.state,
        })
        .collect()
}
