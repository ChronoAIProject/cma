//! Versioned CMA Triggers that open an Agent for the visiting user.

mod agent_source;
mod connections;
mod content;
mod launch;
mod launch_event;
mod launch_initiation;
mod launch_input_protocol;
mod launch_lifecycle;
mod launch_message_evidence;
mod launch_message_progress;
mod launch_progress;
mod launch_resolution_service;
mod launch_service;
mod launch_service_operations;
mod launch_service_support;
mod launch_store;
mod launch_trust;
mod launch_validation;
mod launch_worker_control;
mod launch_worker_replacement;
mod placement;
mod prepared_agent_reuse;
mod preview;
mod reference;
mod requirements;
mod resolution;
mod revision;
mod service_connection_service;
mod standing;
pub mod store;
mod trigger_service;
mod worker_effects;

use serde::{Deserialize, Serialize};

use crate::ids::SubjectId;

pub use crate::ids::{PlacementId, PublicTriggerId, TriggerId};
pub use agent_source::AgentTriggerSource;
pub use connections::{
    BrokerHandoff, BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError,
    ServiceConnectionHandoff,
};
pub use content::{
    AgentReusePolicy, ButtonLabel, Presentation, ReusePolicy, SandboxReusePolicy, Theme,
    TriggerConsent, TriggerContent, TriggerImage, TriggerInlineAgent, TriggerInlineTarget,
    TriggerInlineWorkspace, TriggerQuickMessage, TriggerRepository, TriggerTarget,
    WorkspaceReusePolicy, MAX_FIRST_MESSAGE_BYTES, MAX_QUICK_MESSAGES,
};
pub use launch::{
    Launch, LaunchAuthorityClass, LaunchBlockedReason, LaunchConfirmation, LaunchConsentSurface,
    LaunchFailure, LaunchMessageEvidence, LaunchSeed, LaunchState, LaunchStep, LaunchStepKey,
    LaunchStepStatus,
};
pub use launch_event::{LaunchEvent, LaunchEventKind};
pub use launch_initiation::LaunchInitiation;
pub use launch_input_protocol::{LaunchFirstMessageCommand, LaunchInputProtocol};
pub use launch_service::{
    CreateOrResumeLaunch, LaunchCreateResult, LaunchPolicy, LaunchPolicySource, LaunchService,
    LaunchServiceError, StartLaunch,
};
pub use launch_store::{
    InMemoryLaunchStore, LaunchAdmission, LaunchAdmissionResult, LaunchCommit, LaunchPage,
    LaunchPageBoundary, LaunchPageRequest, LaunchPlacementStats, LaunchStore, LaunchStoreError,
    ServiceConnectionStore, DEFAULT_LAUNCH_EVENT_LIMIT, DEFAULT_LAUNCH_PAGE_SIZE,
    MAX_LAUNCH_EVENT_LIMIT, MAX_LAUNCH_PAGE_SIZE,
};
pub use launch_trust::{LaunchTrustDisclosure, LaunchTrustManifest, LaunchTrustResource};
pub use launch_worker_control::LaunchRequirementRefresh;
pub use placement::{Placement, PlacementUpdate, MAX_LIVE_PLACEMENTS};
pub use preview::{
    TriggerPreview, TriggerSizePreview, TriggerTechnicalPreview, TriggerTrustPreview,
    TriggerTrustPreviewDisclosure,
};
pub use reference::{
    ResolvedTriggerProfiles, TriggerInlineTargetMaterializer, TriggerProfileReader,
    TriggerProfileResolver, TriggerReferenceError,
};
pub use requirements::{
    RequirementsSnapshot, ReviewNeed, ReviewRequirements, ServiceConnectionMetadata,
    ServiceConnectionMode, ServiceRequirement, ServiceRequirementState,
};
pub use resolution::{
    ExistingAgentChoice, LaunchContextReader, LaunchResolution, LaunchResolutionError,
    RepeatChoice, ResolvedResource,
};
pub use revision::{trigger_content_digest, TriggerRevision};
pub use service_connection_service::{
    CreateServiceConnection, ServiceConnectionService, ServiceConnectionServiceError,
};
pub use store::{
    InMemoryTriggerStore, PublicTrigger, TriggerPage, TriggerPageRequest, TriggerStore,
    TriggerStoreError,
};
pub use trigger_service::{TriggerService, TriggerServiceError, TriggerStatistics};
pub use worker_effects::{
    LaunchEffectKind, LaunchEffectRecord, LaunchEffectState, LaunchWorkLease, LaunchWorkState,
    LaunchWorkStep, MAX_LAUNCH_RETRY_DELAY_SECONDS, MAX_LAUNCH_WORK_ATTEMPTS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerAvailability {
    Draft,
    Published,
    Retired,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerStats {
    pub launches_total: u64,
    pub agents_ready_total: u64,
}

impl TriggerStats {
    fn record_launch(&mut self) -> Result<(), TriggerError> {
        self.launches_total = self
            .launches_total
            .checked_add(1)
            .ok_or_else(|| TriggerError::new("trigger_counter_overflow", "stats.launches_total"))?;
        Ok(())
    }

    fn record_agent_ready(&mut self) -> Result<(), TriggerError> {
        if self.agents_ready_total >= self.launches_total {
            return Err(TriggerError::new(
                "agent_ready_without_launch",
                "stats.agents_ready_total",
            ));
        }
        self.agents_ready_total = self.agents_ready_total.checked_add(1).ok_or_else(|| {
            TriggerError::new("trigger_counter_overflow", "stats.agents_ready_total")
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub id: TriggerId,
    pub owner: SubjectId,
    pub draft: TriggerContent,
    pub draft_version: u64,
    #[serde(default)]
    pub revisions: Vec<TriggerRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_id: Option<PublicTriggerId>,
    pub availability: TriggerAvailability,
    #[serde(default)]
    pub placements: Vec<Placement>,
    #[serde(default)]
    pub stats: TriggerStats,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
}

impl Trigger {
    pub fn new(
        id: TriggerId,
        owner: SubjectId,
        draft: TriggerContent,
        now: String,
    ) -> Result<Self, TriggerError> {
        draft.validate()?;
        require_time(&now, "created_at")?;
        let trigger = Self {
            id,
            owner,
            draft,
            draft_version: 1,
            revisions: Vec::new(),
            public_id: None,
            availability: TriggerAvailability::Draft,
            placements: Vec::new(),
            stats: TriggerStats::default(),
            created_at: now.clone(),
            updated_at: now,
            retired_at: None,
            archived_at: None,
        };
        trigger.validate()?;
        Ok(trigger)
    }

    pub fn replace_draft(
        &mut self,
        expected_version: u64,
        draft: TriggerContent,
        updated_at: String,
    ) -> Result<(), TriggerError> {
        self.require_mutable("draft")?;
        if self.draft_version != expected_version {
            return Err(TriggerError::new("trigger_draft_changed", "draft_version"));
        }
        draft.validate()?;
        require_time(&updated_at, "updated_at")?;
        self.draft_version = self
            .draft_version
            .checked_add(1)
            .ok_or_else(|| TriggerError::new("trigger_version_overflow", "draft_version"))?;
        self.draft = draft;
        self.updated_at = updated_at;
        Ok(())
    }

    pub fn publish(&mut self, published_at: String) -> Result<TriggerRevision, TriggerError> {
        self.require_mutable("availability")?;
        self.draft.validate()?;
        let number = u32::try_from(self.revisions.len())
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| TriggerError::new("trigger_revision_overflow", "revisions"))?;
        let revision = TriggerRevision::new(number, self.draft.clone(), published_at.clone())?;
        if self.public_id.is_none() {
            self.public_id = Some(PublicTriggerId::generate());
        }
        self.revisions.push(revision.clone());
        self.availability = TriggerAvailability::Published;
        self.updated_at = published_at;
        self.validate()?;
        Ok(revision)
    }

    pub fn retire(&mut self, retired_at: String) -> Result<(), TriggerError> {
        require_time(&retired_at, "retired_at")?;
        match self.availability {
            TriggerAvailability::Published => {
                self.availability = TriggerAvailability::Retired;
                self.updated_at = retired_at.clone();
                self.retired_at = Some(retired_at);
                Ok(())
            }
            TriggerAvailability::Retired => Ok(()),
            TriggerAvailability::Draft => {
                Err(TriggerError::new("trigger_not_published", "availability"))
            }
        }
    }

    /// Remove the owner's definition while retaining admitted Launch authority.
    pub fn archive(&mut self, archived_at: String) -> Result<(), TriggerError> {
        require_time(&archived_at, "archived_at")?;
        if self.archived_at.is_none() {
            self.retire(archived_at.clone())?;
            self.updated_at = archived_at.clone();
            self.archived_at = Some(archived_at);
        }
        Ok(())
    }

    pub fn add_placement(&mut self, placement: Placement) -> Result<(), TriggerError> {
        self.require_mutable("placements")?;
        placement.validate()?;
        if self
            .placements
            .iter()
            .any(|stored| stored.id == placement.id)
        {
            return Err(TriggerError::new(
                "placement_already_exists",
                "placements.id",
            ));
        }
        if self.live_placements().count() >= MAX_LIVE_PLACEMENTS {
            return Err(TriggerError::new("too_many_placements", "placements"));
        }
        if self
            .live_placements()
            .any(|stored| stored.source_url == placement.source_url)
        {
            return Err(TriggerError::new(
                "placement_source_already_exists",
                "placements.source_url",
            ));
        }
        self.placements.push(placement);
        Ok(())
    }

    pub fn update_placement(
        &mut self,
        id: &PlacementId,
        update: PlacementUpdate,
    ) -> Result<Placement, TriggerError> {
        self.require_mutable("placements")?;
        let normalized = update.normalize()?;
        if self
            .live_placements()
            .any(|stored| &stored.id != id && stored.source_url == normalized.source_url)
        {
            return Err(TriggerError::new(
                "placement_source_already_exists",
                "placements.source_url",
            ));
        }
        let placement = self
            .placements
            .iter_mut()
            .find(|stored| &stored.id == id && stored.retired_at.is_none())
            .ok_or_else(|| TriggerError::new("placement_not_found", "placements.id"))?;
        placement.source_url = normalized.source_url;
        placement.label = normalized.label;
        Ok(placement.clone())
    }

    pub fn retire_placement(
        &mut self,
        id: &PlacementId,
        retired_at: String,
    ) -> Result<Placement, TriggerError> {
        self.require_mutable("placements")?;
        require_time(&retired_at, "placements.retired_at")?;
        let placement = self
            .placements
            .iter_mut()
            .find(|stored| &stored.id == id)
            .ok_or_else(|| TriggerError::new("placement_not_found", "placements.id"))?;
        if placement.retired_at.is_none() {
            placement.retired_at = Some(retired_at);
        }
        Ok(placement.clone())
    }

    pub fn latest_revision(&self) -> Option<&TriggerRevision> {
        self.revisions.last()
    }

    pub fn record_launch(&mut self) -> Result<(), TriggerError> {
        if self.availability != TriggerAvailability::Published {
            return Err(TriggerError::new("trigger_not_published", "availability"));
        }
        self.stats.record_launch()
    }

    pub fn record_agent_ready(&mut self) -> Result<(), TriggerError> {
        self.stats.record_agent_ready()
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        self.draft.validate()?;
        require_prefixed_id(self.id.as_str(), "trg_", "id")?;
        if let Some(public_id) = &self.public_id {
            require_prefixed_id(public_id.as_str(), "pub_", "public_id")?;
        }
        require_time(&self.created_at, "created_at")?;
        require_time(&self.updated_at, "updated_at")?;
        if let Some(retired_at) = &self.retired_at {
            require_time(retired_at, "retired_at")?;
        }
        if let Some(archived_at) = &self.archived_at {
            require_time(archived_at, "archived_at")?;
            if self.availability != TriggerAvailability::Retired {
                return Err(TriggerError::new("invalid_trigger_state", "archived_at"));
            }
        }
        if self.draft_version == 0 {
            return Err(TriggerError::new("invalid_draft_version", "draft_version"));
        }
        let published = !self.revisions.is_empty() && self.public_id.is_some();
        match self.availability {
            TriggerAvailability::Draft if published || self.retired_at.is_some() => {
                return Err(TriggerError::new("invalid_trigger_state", "availability"));
            }
            TriggerAvailability::Published if !published || self.retired_at.is_some() => {
                return Err(TriggerError::new("invalid_trigger_state", "availability"));
            }
            TriggerAvailability::Retired if !published || self.retired_at.is_none() => {
                return Err(TriggerError::new("invalid_trigger_state", "availability"));
            }
            _ => {}
        }
        for (index, revision) in self.revisions.iter().enumerate() {
            revision.validate()?;
            if usize::try_from(revision.revision).ok() != Some(index + 1) {
                return Err(TriggerError::new("invalid_trigger_revision", "revisions"));
            }
        }
        if self.stats.agents_ready_total > self.stats.launches_total {
            return Err(TriggerError::new("invalid_trigger_stats", "stats"));
        }
        let live: Vec<_> = self.live_placements().collect();
        if live.len() > MAX_LIVE_PLACEMENTS {
            return Err(TriggerError::new("too_many_placements", "placements"));
        }
        if live.iter().enumerate().any(|(index, placement)| {
            live[index + 1..]
                .iter()
                .any(|other| placement.source_url == other.source_url)
        }) {
            return Err(TriggerError::new(
                "placement_source_already_exists",
                "placements.source_url",
            ));
        }
        self.placements.iter().try_for_each(Placement::validate)
    }

    fn live_placements(&self) -> impl Iterator<Item = &Placement> {
        self.placements
            .iter()
            .filter(|placement| placement.retired_at.is_none())
    }

    fn require_mutable(&self, field: &'static str) -> Result<(), TriggerError> {
        if self.availability == TriggerAvailability::Retired {
            Err(TriggerError::new("trigger_retired", field))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {field}")]
pub struct TriggerError {
    pub code: &'static str,
    pub field: &'static str,
}

impl TriggerError {
    pub const fn new(code: &'static str, field: &'static str) -> Self {
        Self { code, field }
    }
}

fn require_time(value: &str, field: &'static str) -> Result<(), TriggerError> {
    if value.trim().is_empty() {
        Err(TriggerError::new("invalid_timestamp", field))
    } else {
        Ok(())
    }
}

fn require_prefixed_id(value: &str, prefix: &str, field: &'static str) -> Result<(), TriggerError> {
    if value.starts_with(prefix) && value.len() > prefix.len() {
        Ok(())
    } else {
        Err(TriggerError::new("invalid_trigger_id", field))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod launch_tests;

#[cfg(test)]
mod launch_progress_tests;

#[cfg(test)]
mod launch_store_tests;

#[cfg(test)]
mod service_connection_service_tests;

#[cfg(test)]
mod trigger_service_tests;

#[cfg(test)]
mod launch_worker_control_tests;

#[cfg(test)]
mod launch_service_tests;

#[cfg(test)]
mod archive_tests;
