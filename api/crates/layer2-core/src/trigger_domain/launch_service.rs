use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::profiles::{SandboxProfileStore, WorkspaceProfileStore};
use crate::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};
use crate::{now_rfc3339, Caller, Capability, LaunchId, SubjectId};

use super::launch_service_support::{
    append_resolution_events, connection_error, create_result, request_fingerprint,
    require_launcher, store_error, trigger_error, validate_create_request,
    validate_requirement_identity,
};
use super::launch_trust::LaunchTrustManifest;
use super::{
    ExistingAgentChoice, Launch, LaunchAdmission, LaunchAuthorityClass, LaunchConfirmation,
    LaunchConsentSurface, LaunchContextReader, LaunchEventKind, LaunchResolution, LaunchSeed,
    LaunchState, LaunchStore, PublicTrigger, PublicTriggerId, RepeatChoice, RequirementsSnapshot,
    ReviewRequirements, ServiceConnectionBroker, TriggerAvailability, TriggerProfileReader,
    TriggerStore,
};

pub(super) const MAX_EXISTING_MESSAGE_EXCERPT_SCALARS: usize = 140;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOrResumeLaunch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<super::PlacementId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referrer_origin: Option<String>,
    pub start_if_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<RepeatChoice>,
    #[serde(skip)]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartLaunch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_confirmed_digest: Option<String>,
    #[serde(default)]
    pub acknowledgements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCreateResult {
    pub launch: Launch,
    pub created: bool,
}

impl LaunchCreateResult {
    pub const fn accepted(&self) -> bool {
        !matches!(self.launch.state, LaunchState::Draft)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchPolicy {
    pub enabled: bool,
    pub orchestration_paused: bool,
}

#[async_trait]
pub trait LaunchPolicySource: Send + Sync {
    async fn current_launch_policy(&self) -> Result<LaunchPolicy, LaunchServiceError>;
}

#[async_trait]
impl LaunchPolicySource for LaunchPolicy {
    async fn current_launch_policy(&self) -> Result<LaunchPolicy, LaunchServiceError> {
        Ok(*self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchServiceError {
    #[error("account deletion is in progress")]
    AccountDeletionInProgress,
    #[error("daily visitor Launch allowance exhausted")]
    VisitorQuota {
        limit: u32,
        reset_at: std::time::SystemTime,
    },
    #[error("renewable Sandbox credential setup is required")]
    CredentialSetupRequired,
    #[error("forbidden")]
    Forbidden,
    #[error("Trigger launches are disabled")]
    Disabled,
    #[error("published Trigger not found")]
    TriggerNotFound,
    #[error("Trigger is retired")]
    TriggerRetired,
    #[error("a repeat choice is required")]
    RepeatChoiceRequired { existing: ExistingAgentChoice },
    #[error("launch not found")]
    NotFound,
    #[error("launch is not startable")]
    NotStartable,
    #[error("service requirements remain unsatisfied")]
    RequirementsNotMet { services: Vec<crate::ServiceId> },
    #[error("trust confirmation does not match the reviewed digest")]
    TrustConfirmationMismatch,
    #[error("invalid launch request: {0}")]
    InvalidRequest(String),
    #[error("Idempotency-Key is invalid")]
    InvalidIdempotencyKey,
    #[error("Idempotency-Key is already bound to different launch input")]
    IdempotencyKeyReused,
    #[error("launch event cursor is invalid")]
    InvalidCursor,
    #[error("launch changed while the operation was committed")]
    Conflict,
    #[error("model budget refused the Launch: {0}")]
    Budget(crate::budget::BudgetError),
    #[error("launch dependency is unavailable: {0}")]
    Unavailable(String),
    #[error("launch data is not usable: {0}")]
    Corrupt(String),
}

impl LaunchServiceError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AccountDeletionInProgress => "account_deletion_in_progress",
            Self::VisitorQuota { .. } => "resource_quota",
            Self::CredentialSetupRequired => "sandbox_credential_setup_required",
            Self::Forbidden => "forbidden",
            Self::Disabled => "triggers_disabled",
            Self::TriggerNotFound => "trigger_not_found",
            Self::TriggerRetired => "trigger_retired",
            Self::RepeatChoiceRequired { .. } => "trigger_repeat_choice_required",
            Self::NotFound => "launch_not_found",
            Self::NotStartable => "launch_not_startable",
            Self::RequirementsNotMet { .. } => "launch_requirements_not_met",
            Self::TrustConfirmationMismatch => "trust_confirmation_mismatch",
            Self::InvalidRequest(_) => "invalid_launch_request",
            Self::InvalidIdempotencyKey => "invalid_idempotency_key",
            Self::IdempotencyKeyReused => "idempotency_key_reused",
            Self::InvalidCursor => "invalid_launch_cursor",
            Self::Conflict => "launch_changed",
            Self::Budget(reason) => reason.code(),
            Self::Unavailable(_) => "launch_service_unavailable",
            Self::Corrupt(_) => "stored_launch_corrupt",
        }
    }
}

pub struct LaunchService {
    pub(super) standing: Arc<dyn crate::account_deletion::AccountStanding>,
    pub(super) common_first_messages: bool,
    pub(super) definitions: Arc<dyn TriggerStore>,
    pub(super) profile_reader: Arc<dyn TriggerProfileReader>,
    pub(super) sandbox_profiles: Arc<dyn SandboxProfileStore>,
    pub(super) workspace_profiles: Arc<dyn WorkspaceProfileStore>,
    pub(super) launches: Arc<dyn LaunchStore>,
    pub(super) contexts: Arc<dyn LaunchContextReader>,
    pub(super) connections: Arc<dyn ServiceConnectionBroker>,
    pub(super) policy: Arc<dyn LaunchPolicySource>,
    pub(super) credentials: Arc<dyn SandboxCredentialPrerequisite>,
}

impl LaunchService {
    pub fn new<T, P, L, C, B, S>(
        definitions: Arc<T>,
        profiles: Arc<P>,
        launches: Arc<L>,
        contexts: Arc<C>,
        connections: Arc<B>,
        policy: Arc<S>,
        credentials: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Arc<Self>
    where
        T: TriggerStore + 'static,
        P: TriggerProfileReader + SandboxProfileStore + WorkspaceProfileStore + 'static,
        L: LaunchStore + 'static,
        C: LaunchContextReader + 'static,
        B: ServiceConnectionBroker + 'static,
        S: LaunchPolicySource + 'static,
    {
        let profile_reader: Arc<dyn TriggerProfileReader> = profiles.clone();
        let sandbox_profiles: Arc<dyn SandboxProfileStore> = profiles.clone();
        let workspace_profiles: Arc<dyn WorkspaceProfileStore> = profiles;
        Arc::new(Self {
            common_first_messages: false,
            standing: Arc::new(crate::account_deletion::AvailableAccounts),
            definitions,
            profile_reader,
            sandbox_profiles,
            workspace_profiles,
            launches,
            contexts,
            connections,
            policy,
            credentials,
        })
    }

    pub(super) async fn require_sandbox_credentials(
        &self,
        owner: &SubjectId,
    ) -> Result<(), LaunchServiceError> {
        self.credentials
            .check(owner)
            .await
            .map_err(|error| match error {
                SandboxCredentialPrerequisiteError::SetupRequired => {
                    LaunchServiceError::CredentialSetupRequired
                }
                SandboxCredentialPrerequisiteError::ConnectionsRequired { services } => {
                    LaunchServiceError::RequirementsNotMet { services }
                }
                SandboxCredentialPrerequisiteError::Unavailable(detail) => {
                    LaunchServiceError::Unavailable(detail)
                }
            })
    }

    pub async fn create_or_resume(
        &self,
        caller: &Caller,
        public_id: &PublicTriggerId,
        request: CreateOrResumeLaunch,
    ) -> Result<LaunchCreateResult, LaunchServiceError> {
        self.create_with_initiator(caller, None, public_id, request)
            .await
    }

    pub async fn create_or_resume_as(
        &self,
        context: &crate::resource_access::CallerContext,
        public_id: &PublicTriggerId,
        request: CreateOrResumeLaunch,
    ) -> Result<LaunchCreateResult, LaunchServiceError> {
        if !context.validate_principal() {
            return Err(LaunchServiceError::Forbidden);
        }
        self.create_with_initiator(&context.caller, Some(context), public_id, request)
            .await
    }

    async fn create_with_initiator(
        &self,
        caller: &Caller,
        context: Option<&crate::resource_access::CallerContext>,
        public_id: &PublicTriggerId,
        request: CreateOrResumeLaunch,
    ) -> Result<LaunchCreateResult, LaunchServiceError> {
        require_launcher(caller)?;
        validate_create_request(&request)?;
        let command_key_hash =
            super::launch_service_support::command_hash(&request.idempotency_key);
        let command_fingerprint = request_fingerprint(public_id, &request)?;
        if let Some(replay) = self
            .replayed_command(
                &caller.subject,
                public_id,
                &command_key_hash,
                &command_fingerprint,
            )
            .await?
        {
            return Ok(replay);
        }
        let result = async {
            if !self.policy.current_launch_policy().await?.enabled {
                return Err(LaunchServiceError::Disabled);
            }
            let trigger = self.public_trigger(public_id).await?;
            let profiles = self
                .profile_reader
                .read_trigger_profiles(&trigger.revision.content.target)
                .await
                .map_err(|error| LaunchServiceError::Unavailable(error.to_string()))?;
            let placement_id = self
                .validated_placement(&trigger, request.placement_id.as_ref())
                .await?;
            let resolved = self
                .resolve(caller, &trigger, &profiles, request.choice)
                .await?;
            let trust = LaunchTrustManifest::resolve(
                self.sandbox_profiles.as_ref(),
                self.workspace_profiles.as_ref(),
                &profiles,
                &resolved.resolution,
                &caller.subject,
            )
            .await?;
            if resolved.resolution.sandbox.is_pending() {
                self.require_sandbox_credentials(&caller.subject).await?;
            }
            let checked_at = now_rfc3339();
            let required_services = self
                .required_services(&caller.subject, &resolved.resolution, &profiles)
                .await?;
            let requirements = RequirementsSnapshot {
                services: self
                    .connections
                    .requirements(&caller.subject, &required_services)
                    .await
                    .map_err(connection_error)?,
                checked_at,
            };
            requirements
                .validate()
                .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
            validate_requirement_identity(&required_services, &requirements)?;
            let review = ReviewRequirements::build(
                &requirements,
                trust.confirmation_digest.clone(),
                trigger.revision.content.consent.always_review,
                false,
            )
            .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
            let now = now_rfc3339();
            let mut launch = Launch::new(LaunchSeed {
                trigger_id: trigger.trigger_id.clone(),
                revision_digest: trigger.revision.digest.clone(),
                public_id: trigger.public_id.clone(),
                owner: caller.subject.clone(),
                authority: if caller.authorization.allows(Capability::CreateSandbox) {
                    LaunchAuthorityClass::SandboxCreator
                } else {
                    LaunchAuthorityClass::Visitor
                },
                placement_id,
                referrer_origin: request.referrer_origin.clone(),
                requirements,
                review,
                trust,
                resolution: resolved.resolution,
                has_first_message: !resolved.existing_agent
                    && !trigger.revision.content.first_message.is_empty(),
                command_key_hash: command_key_hash.clone(),
                command_fingerprint: command_fingerprint.clone(),
                created_at: now.clone(),
            })
            .map_err(|error| LaunchServiceError::InvalidRequest(error.to_string()))?;
            launch.initiation = context
                .map(|context| super::LaunchInitiation::capture(context, &launch))
                .transpose()?;
            if self.common_first_messages {
                launch.input_protocol = Some(super::LaunchInputProtocol::capture(&launch)?);
            }
            let mut events = vec![LaunchEventKind::Created];
            if let Some(choice) = request.choice {
                events.push(LaunchEventKind::RepeatChoiceRecorded { choice });
            }
            append_resolution_events(&launch, &mut events);
            // Starting a no-review Trigger explicitly accepts its frozen configuration.
            // Connection requirements and publisher-required review remain blocking.
            if request.start_if_ready && !trigger.revision.content.consent.always_review {
                launch
                    .confirm_landing_start(LaunchConfirmation {
                        surface: LaunchConsentSurface::Landing,
                        trust_confirmed_digest: launch.trust.confirmation_digest.clone(),
                        acknowledgements: Vec::new(),
                        confirmed_at: now.clone(),
                    })
                    .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
                events.push(LaunchEventKind::ConfirmationRecorded);
                if launch.review.can_start_now {
                    launch
                        .start_confirmed(now)
                        .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
                    events.push(LaunchEventKind::StateChanged {
                        from: LaunchState::Draft,
                        to: LaunchState::Resolving,
                    });
                }
            }
            let admitted = self
                .launches
                .admit_launch(LaunchAdmission {
                    launch,
                    trigger,
                    events,
                })
                .await
                .map_err(store_error)?;
            Ok(create_result(admitted))
        }
        .await;
        match result {
            Ok(created) => Ok(created),
            Err(original) => match self
                .replayed_command(
                    &caller.subject,
                    public_id,
                    &command_key_hash,
                    &command_fingerprint,
                )
                .await
            {
                Ok(Some(replay)) => Ok(replay),
                Ok(None) => Err(original),
                Err(lookup) => Err(lookup),
            },
        }
    }

    async fn replayed_command(
        &self,
        owner: &SubjectId,
        public_id: &PublicTriggerId,
        command_key_hash: &str,
        command_fingerprint: &str,
    ) -> Result<Option<LaunchCreateResult>, LaunchServiceError> {
        let Some(existing) = self
            .launches
            .launch_by_command(owner, public_id, command_key_hash)
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        if existing.command_fingerprint != command_fingerprint {
            return Err(LaunchServiceError::IdempotencyKeyReused);
        }
        Ok(Some(LaunchCreateResult {
            launch: existing,
            created: false,
        }))
    }

    pub(super) async fn public_trigger(
        &self,
        public_id: &PublicTriggerId,
    ) -> Result<PublicTrigger, LaunchServiceError> {
        let trigger = self
            .definitions
            .trigger_by_public_id(public_id)
            .await
            .map_err(trigger_error)?
            .ok_or(LaunchServiceError::TriggerNotFound)?;
        self.require_public_owner(&trigger.owner).await?;
        if trigger.availability != TriggerAvailability::Published {
            return Err(LaunchServiceError::TriggerRetired);
        }
        Ok(trigger)
    }

    async fn validated_placement(
        &self,
        trigger: &PublicTrigger,
        requested: Option<&super::PlacementId>,
    ) -> Result<Option<super::PlacementId>, LaunchServiceError> {
        let Some(requested) = requested else {
            return Ok(None);
        };
        let aggregate = self
            .definitions
            .trigger(&trigger.trigger_id, &trigger.owner)
            .await
            .map_err(trigger_error)?
            .ok_or_else(|| LaunchServiceError::Corrupt("published Trigger is missing".into()))?;
        aggregate
            .placements
            .iter()
            .find(|placement| &placement.id == requested && placement.retired_at.is_none())
            .map(|placement| Some(placement.id.clone()))
            .ok_or_else(|| LaunchServiceError::InvalidRequest("placement_id is unavailable".into()))
    }

    pub(super) async fn owned(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Launch, LaunchServiceError> {
        self.launches
            .launch(id, owner)
            .await
            .map_err(store_error)?
            .ok_or(LaunchServiceError::NotFound)
    }
}

pub(super) struct ResolvedLaunch {
    pub resolution: LaunchResolution,
    pub existing_agent: bool,
}

impl ResolvedLaunch {
    pub(super) fn pending() -> Self {
        Self::new(LaunchResolution::pending(), false)
    }

    pub(super) fn new(resolution: LaunchResolution, existing_agent: bool) -> Self {
        Self {
            resolution,
            existing_agent,
        }
    }
}
