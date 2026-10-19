use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use axum::Router;
use layer2_core::{
    ids::{AgentProfileId, SandboxProfileId, WorkspaceProfileId},
    profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef},
    trigger_domain::{
        AgentReusePolicy, ButtonLabel, Placement, PlacementId, PlacementUpdate, Presentation,
        PublicTrigger, PublicTriggerId, ReusePolicy, SandboxReusePolicy, Theme, Trigger,
        TriggerAvailability, TriggerConsent, TriggerContent, TriggerId, TriggerPage,
        TriggerPageRequest, TriggerPreview, TriggerRevision, TriggerStats, TriggerStore,
        TriggerStoreError, TriggerTarget, WorkspaceReusePolicy,
    },
    SubjectId,
};

use super::{trigger_assets_router, TriggerAssetsState};

pub(super) const OWNER: &str = "publisher";
pub(super) const OTHER_OWNER: &str = "somebody-else";
pub(super) const TRIGGER_ID: &str = "trg_example";
pub(super) const PUBLIC_ID: &str = "pub_example";
pub(super) const PLACEMENT_ID: &str = "plc_readme";
pub(super) const ORIGIN: &str = "https://cma.example";
pub(super) const OWNER_SECRET: &str = "OWNER_PRIVATE_MARKER";

pub(super) struct Fixture {
    pub(super) trigger: Trigger,
    pub(super) placement: Placement,
    pub(super) store: Arc<ReadOnlyTriggerStore>,
}

impl Fixture {
    pub(super) fn no_review() -> Self {
        let mut fixture = Self::published();
        fixture.trigger.draft.consent.always_review = false;
        fixture
            .trigger
            .publish("2026-09-05T02:00:00Z".into())
            .unwrap();
        fixture.store = Arc::new(ReadOnlyTriggerStore::new(fixture.trigger.clone()));
        fixture
    }
    pub(super) fn published() -> Self {
        Self::new(TriggerAvailability::Published, false)
    }

    pub(super) fn retired_trigger() -> Self {
        Self::new(TriggerAvailability::Retired, false)
    }

    pub(super) fn retired_placement() -> Self {
        Self::new(TriggerAvailability::Published, true)
    }

    pub(super) fn draft() -> Self {
        Self::new(TriggerAvailability::Draft, false)
    }

    fn new(availability: TriggerAvailability, retire_placement: bool) -> Self {
        let mut placement = Placement {
            id: PlacementId(PLACEMENT_ID.into()),
            source_url: "https://docs.example/readme".into(),
            label: OWNER_SECRET.into(),
            retired_at: None,
        };
        let mut trigger = Trigger::new(
            TriggerId(TRIGGER_ID.into()),
            SubjectId(OWNER.into()),
            content(),
            "2026-09-05T00:00:00Z".into(),
        )
        .expect("fixture Trigger is valid");
        trigger
            .add_placement(placement.clone())
            .expect("fixture placement is valid");
        if retire_placement {
            trigger
                .retire_placement(&placement.id, "2026-09-05T03:00:00Z".into())
                .expect("fixture placement retires");
            placement.retired_at = Some("2026-09-05T03:00:00Z".into());
        }
        if availability != TriggerAvailability::Draft {
            trigger
                .publish("2026-09-05T01:00:00Z".into())
                .expect("fixture Trigger publishes");
            trigger.public_id = Some(PublicTriggerId(PUBLIC_ID.into()));
        }
        if availability == TriggerAvailability::Retired {
            trigger
                .retire("2026-09-05T04:00:00Z".into())
                .expect("fixture Trigger retires");
        }
        let store = Arc::new(ReadOnlyTriggerStore::new(trigger.clone()));
        Self {
            trigger,
            placement,
            store,
        }
    }

    pub(super) fn router(&self) -> Router {
        let store: Arc<dyn TriggerStore> = self.store.clone();
        let state = TriggerAssetsState::new(store, ORIGIN).expect("fixture origin is valid");
        trigger_assets_router(state)
    }
}

fn content() -> TriggerContent {
    TriggerContent {
        presentation: Presentation {
            title: format!("Private title {OWNER_SECRET}"),
            description: format!("Private description {OWNER_SECRET}"),
            button_label: ButtonLabel::OpenInCma,
            theme: Theme::Auto,
            signup_hint: Some(OWNER_SECRET.into()),
        },
        target: TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(SandboxProfileId("sbp_example".into()), 1),
            workspace_profile: WorkspaceProfileRef::new(
                WorkspaceProfileId("wkp_example".into()),
                1,
            ),
            agent_profile: AgentProfileRef::new(AgentProfileId("agp_example".into()), 1),
            inline: None,
        },
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::Ask,
        },
        first_message: OWNER_SECRET.into(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: true,
        },
    }
}

pub(super) struct ReadOnlyTriggerStore {
    trigger: Trigger,
    public_lookups: AtomicUsize,
}

impl ReadOnlyTriggerStore {
    fn new(trigger: Trigger) -> Self {
        Self {
            trigger,
            public_lookups: AtomicUsize::new(0),
        }
    }

    pub(super) fn public_lookups(&self) -> usize {
        self.public_lookups.load(Ordering::Relaxed)
    }

    fn public_trigger(&self, id: &PublicTriggerId) -> Option<PublicTrigger> {
        let public_id = self.trigger.public_id.as_ref()?.clone();
        if &public_id != id {
            return None;
        }
        Some(PublicTrigger {
            trigger_id: self.trigger.id.clone(),
            public_id,
            owner: self.trigger.owner.clone(),
            availability: self.trigger.availability,
            revision: self.trigger.latest_revision()?.clone(),
        })
    }
}

#[async_trait]
impl TriggerStore for ReadOnlyTriggerStore {
    async fn create_trigger(&self, _trigger: Trigger) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::Duplicate)
    }

    async fn trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
    ) -> Result<Option<Trigger>, TriggerStoreError> {
        Ok((&self.trigger.id == id && &self.trigger.owner == owner).then(|| self.trigger.clone()))
    }

    async fn delete_trigger(
        &self,
        _id: &TriggerId,
        _owner: &SubjectId,
        _archived_at: String,
    ) -> Result<(), TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        let revision = (&self.trigger.id == id)
            .then_some(&self.trigger)
            .and_then(|trigger| {
                trigger
                    .revisions
                    .iter()
                    .find(|revision| revision.digest == digest)
            })
            .cloned();
        if let Some(revision) = &revision {
            revision
                .validate()
                .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))?;
        }
        Ok(revision)
    }

    async fn save_draft(
        &self,
        _id: &TriggerId,
        _owner: &SubjectId,
        _expected_draft_version: u64,
        _draft: TriggerContent,
        _updated_at: String,
    ) -> Result<Trigger, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn publish_trigger(
        &self,
        _id: &TriggerId,
        _owner: &SubjectId,
        _expected_draft_version: u64,
        _published_at: String,
    ) -> Result<PublicTrigger, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn trigger_by_public_id(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<PublicTrigger>, TriggerStoreError> {
        self.public_lookups.fetch_add(1, Ordering::Relaxed);
        Ok(self.public_trigger(id))
    }

    async fn preview_by_public_id(
        &self,
        _id: &PublicTriggerId,
        _publisher_display_name: String,
    ) -> Result<Option<TriggerPreview>, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn list_own_triggers(
        &self,
        owner: &SubjectId,
        _page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Trigger>, TriggerStoreError> {
        Ok(TriggerPage {
            items: (&self.trigger.owner == owner)
                .then(|| self.trigger.clone())
                .into_iter()
                .collect(),
            next: None,
        })
    }

    async fn retire_trigger(
        &self,
        _id: &TriggerId,
        _owner: &SubjectId,
        _retired_at: String,
    ) -> Result<Trigger, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn create_placement(
        &self,
        _trigger_id: &TriggerId,
        _owner: &SubjectId,
        _source_url: String,
        _label: String,
    ) -> Result<Placement, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
    ) -> Result<Option<Placement>, TriggerStoreError> {
        Ok(self.trigger(trigger_id, owner).await?.and_then(|trigger| {
            trigger
                .placements
                .into_iter()
                .find(|placement| &placement.id == placement_id)
        }))
    }

    async fn list_placements(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        include_retired: bool,
    ) -> Result<Vec<Placement>, TriggerStoreError> {
        Ok(self
            .trigger(trigger_id, owner)
            .await?
            .into_iter()
            .flat_map(|trigger| trigger.placements)
            .filter(|placement| include_retired || placement.retired_at.is_none())
            .collect())
    }

    async fn update_placement(
        &self,
        _trigger_id: &TriggerId,
        _owner: &SubjectId,
        _placement_id: &PlacementId,
        _update: PlacementUpdate,
    ) -> Result<Placement, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn retire_placement(
        &self,
        _trigger_id: &TriggerId,
        _owner: &SubjectId,
        _placement_id: &PlacementId,
        _retired_at: String,
    ) -> Result<Placement, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn record_launch(&self, _id: &TriggerId) -> Result<TriggerStats, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn record_agent_ready(&self, _id: &TriggerId) -> Result<TriggerStats, TriggerStoreError> {
        Err(TriggerStoreError::NotFound)
    }

    async fn guard_sandbox_profile_retirement(
        &self,
        _id: &SandboxProfileId,
    ) -> Result<(), TriggerStoreError> {
        Ok(())
    }

    async fn guard_workspace_profile_retirement(
        &self,
        _id: &WorkspaceProfileId,
    ) -> Result<(), TriggerStoreError> {
        Ok(())
    }

    async fn guard_agent_profile_retirement(
        &self,
        _id: &AgentProfileId,
    ) -> Result<(), TriggerStoreError> {
        Ok(())
    }
}
