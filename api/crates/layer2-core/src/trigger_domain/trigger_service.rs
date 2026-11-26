use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::{now_rfc3339, Caller, Capability, StoreError, SubjectId, UserDirectory};

use super::{
    LaunchPlacementStats, LaunchStore, LaunchStoreError, Placement, PlacementId, PublicTrigger,
    PublicTriggerId, Trigger, TriggerContent, TriggerError, TriggerId,
    TriggerInlineTargetMaterializer, TriggerPage, TriggerPageRequest, TriggerPreview,
    TriggerReferenceError, TriggerRevision, TriggerStats, TriggerStore, TriggerStoreError,
};

mod inline;

const PUBLISHER_FALLBACK: &str = "CMA publisher";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerStatistics {
    pub totals: TriggerStats,
    pub placements: Vec<LaunchPlacementStats>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TriggerServiceError {
    #[error("forbidden")]
    Forbidden,
    #[error("trigger not found")]
    NotFound,
    #[error("trigger already exists")]
    Duplicate,
    #[error("trigger draft changed: expected {expected}, found {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("trigger has already been published")]
    AlreadyPublished,
    #[error(transparent)]
    Reference(TriggerReferenceError),
    #[error("profile is referenced by published triggers: {trigger_ids:?}")]
    ProfileReferenced { trigger_ids: Vec<TriggerId> },
    #[error(transparent)]
    Validation(TriggerError),
    #[error("trigger dependency is unavailable: {0}")]
    Unavailable(String),
    #[error("trigger data is not usable: {0}")]
    Corrupt(String),
}

impl TriggerServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::NotFound => "trigger_not_found",
            Self::Duplicate => "trigger_already_exists",
            Self::Conflict { .. } => "trigger_draft_changed",
            Self::AlreadyPublished => "trigger_already_published",
            Self::Reference(error) => error.code(),
            Self::ProfileReferenced { .. } => "profile_referenced_by_published_trigger",
            Self::Validation(error) => error.code,
            Self::Unavailable(_) => "trigger_service_unavailable",
            Self::Corrupt(_) => "stored_trigger_corrupt",
        }
    }
}

impl From<TriggerStoreError> for TriggerServiceError {
    fn from(error: TriggerStoreError) -> Self {
        match error {
            TriggerStoreError::NotFound => Self::NotFound,
            TriggerStoreError::Duplicate => Self::Duplicate,
            TriggerStoreError::Conflict { expected, actual } => Self::Conflict { expected, actual },
            TriggerStoreError::AlreadyPublished => Self::AlreadyPublished,
            TriggerStoreError::Reference(error) => Self::Reference(error),
            TriggerStoreError::ProfileReferenced { trigger_ids } => {
                Self::ProfileReferenced { trigger_ids }
            }
            TriggerStoreError::Unavailable(detail) => Self::Unavailable(detail),
            TriggerStoreError::Corrupt(detail) => Self::Corrupt(detail),
            TriggerStoreError::Validation(error) => Self::Validation(error),
        }
    }
}

pub struct TriggerService {
    pub(super) standing: Arc<dyn crate::account_deletion::AccountStanding>,
    pub(super) definitions: Arc<dyn TriggerStore>,
    pub(super) launches: Arc<dyn LaunchStore>,
    directory: Arc<UserDirectory>,
    inline_materializer: Option<Arc<dyn TriggerInlineTargetMaterializer>>,
}

impl TriggerService {
    pub fn new<T, L>(
        definitions: Arc<T>,
        launches: Arc<L>,
        directory: Arc<UserDirectory>,
    ) -> Arc<Self>
    where
        T: TriggerStore + 'static,
        L: LaunchStore + 'static,
    {
        Arc::new(Self {
            definitions,
            launches,
            directory,
            inline_materializer: None,
            standing: Arc::new(crate::account_deletion::AvailableAccounts),
        })
    }

    pub fn new_with_inline_materializer<T, L, M>(
        definitions: Arc<T>,
        launches: Arc<L>,
        directory: Arc<UserDirectory>,
        inline_materializer: Arc<M>,
    ) -> Arc<Self>
    where
        T: TriggerStore + 'static,
        L: LaunchStore + 'static,
        M: TriggerInlineTargetMaterializer + 'static,
    {
        Arc::new(Self {
            definitions,
            launches,
            directory,
            inline_materializer: Some(inline_materializer),
            standing: Arc::new(crate::account_deletion::AvailableAccounts),
        })
    }

    pub async fn create(
        &self,
        caller: &Caller,
        draft: TriggerContent,
    ) -> Result<Trigger, TriggerServiceError> {
        self.create_with_credential(caller, draft, None).await
    }

    pub async fn create_with_credential(
        &self,
        caller: &Caller,
        draft: TriggerContent,
        delegated_credential: Option<&str>,
    ) -> Result<Trigger, TriggerServiceError> {
        require_publisher(caller)?;
        let draft = self
            .materialize_inline(caller, draft, None, delegated_credential)
            .await?;
        let trigger = Trigger::new(
            TriggerId::generate(),
            caller.subject.clone(),
            draft,
            now_rfc3339(),
        )
        .map_err(TriggerServiceError::Validation)?;
        self.definitions
            .create_trigger(trigger.clone())
            .await
            .map_err(TriggerServiceError::from)?;
        Ok(trigger)
    }

    pub async fn list(
        &self,
        caller: &Caller,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Trigger>, TriggerServiceError> {
        require_publisher(caller)?;
        page.validate().map_err(TriggerServiceError::from)?;
        self.definitions
            .list_own_triggers(&caller.subject, page)
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn read(
        &self,
        caller: &Caller,
        id: &TriggerId,
    ) -> Result<Trigger, TriggerServiceError> {
        self.owned(id, &caller.subject).await
    }

    pub async fn update_draft(
        &self,
        caller: &Caller,
        id: &TriggerId,
        expected_draft_version: u64,
        draft: TriggerContent,
    ) -> Result<Trigger, TriggerServiceError> {
        self.update_draft_with_credential(caller, id, expected_draft_version, draft, None)
            .await
    }

    pub async fn update_draft_with_credential(
        &self,
        caller: &Caller,
        id: &TriggerId,
        expected_draft_version: u64,
        draft: TriggerContent,
        delegated_credential: Option<&str>,
    ) -> Result<Trigger, TriggerServiceError> {
        // Verify ownership before materializing inline profiles so a rejected update
        // cannot leave private internal profile rows behind.
        let current = self.owned(id, &caller.subject).await?;
        let draft = self
            .materialize_inline(
                caller,
                draft,
                Some(&current.draft.target),
                delegated_credential,
            )
            .await?;
        self.definitions
            .save_draft(
                id,
                &caller.subject,
                expected_draft_version,
                draft,
                now_rfc3339(),
            )
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn delete(&self, caller: &Caller, id: &TriggerId) -> Result<(), TriggerServiceError> {
        self.definitions
            .delete_trigger(id, &caller.subject, now_rfc3339())
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn publish(
        &self,
        caller: &Caller,
        id: &TriggerId,
        expected_draft_version: u64,
    ) -> Result<PublicTrigger, TriggerServiceError> {
        self.definitions
            .publish_trigger(id, &caller.subject, expected_draft_version, now_rfc3339())
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn retire(
        &self,
        caller: &Caller,
        id: &TriggerId,
    ) -> Result<Trigger, TriggerServiceError> {
        self.definitions
            .retire_trigger(id, &caller.subject, now_rfc3339())
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn revisions(
        &self,
        caller: &Caller,
        id: &TriggerId,
    ) -> Result<Vec<TriggerRevision>, TriggerServiceError> {
        Ok(self.read(caller, id).await?.revisions)
    }

    pub async fn create_placement(
        &self,
        caller: &Caller,
        id: &TriggerId,
        source_url: String,
        label: String,
    ) -> Result<Placement, TriggerServiceError> {
        self.definitions
            .create_placement(id, &caller.subject, source_url, label)
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn placements(
        &self,
        caller: &Caller,
        id: &TriggerId,
        include_retired: bool,
    ) -> Result<Vec<Placement>, TriggerServiceError> {
        self.definitions
            .list_placements(id, &caller.subject, include_retired)
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn retire_placement(
        &self,
        caller: &Caller,
        id: &TriggerId,
        placement_id: &PlacementId,
    ) -> Result<Placement, TriggerServiceError> {
        self.definitions
            .retire_placement(id, &caller.subject, placement_id, now_rfc3339())
            .await
            .map_err(TriggerServiceError::from)
    }

    pub async fn statistics(
        &self,
        caller: &Caller,
        id: &TriggerId,
    ) -> Result<TriggerStatistics, TriggerServiceError> {
        let trigger = self.owned(id, &caller.subject).await?;
        let stored = self
            .launches
            .placement_stats(id)
            .await
            .map_err(launch_store_error)?;
        reconcile_statistics(&trigger, stored)
    }

    pub async fn preview(
        &self,
        public_id: &PublicTriggerId,
    ) -> Result<TriggerPreview, TriggerServiceError> {
        let public = self
            .definitions
            .trigger_by_public_id(public_id)
            .await
            .map_err(TriggerServiceError::from)?
            .ok_or(TriggerServiceError::NotFound)?;
        self.require_public_owner(&public.owner).await?;
        let publisher = self
            .directory
            .user(&public.owner)
            .await
            .map_err(directory_error)?;
        if publisher
            .as_ref()
            .is_some_and(|record| record.identity.subject != public.owner)
        {
            return Err(TriggerServiceError::Corrupt(
                "publisher directory lookup crossed its subject boundary".into(),
            ));
        }
        let display_name = publisher
            .and_then(|record| record.identity.display_name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| PUBLISHER_FALLBACK.to_owned());
        self.definitions
            .preview_by_public_id(public_id, display_name)
            .await
            .map_err(TriggerServiceError::from)?
            .ok_or_else(|| {
                TriggerServiceError::Corrupt(
                    "public Trigger disappeared while its preview was projected".into(),
                )
            })
    }

    async fn owned(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
    ) -> Result<Trigger, TriggerServiceError> {
        self.definitions
            .trigger(id, owner)
            .await
            .map_err(TriggerServiceError::from)?
            .ok_or(TriggerServiceError::NotFound)
    }
}

fn require_publisher(caller: &Caller) -> Result<(), TriggerServiceError> {
    caller
        .authorization
        .allows(Capability::PublishTriggers)
        .then_some(())
        .ok_or(TriggerServiceError::Forbidden)
}

fn directory_error(error: StoreError) -> TriggerServiceError {
    match error {
        StoreError::Malformed(detail) => TriggerServiceError::Corrupt(detail),
        other => TriggerServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn launch_store_error(error: LaunchStoreError) -> TriggerServiceError {
    match error {
        LaunchStoreError::Unavailable(detail) => TriggerServiceError::Unavailable(detail),
        LaunchStoreError::Corrupt(detail) => TriggerServiceError::Corrupt(detail),
        LaunchStoreError::Validation(error) => TriggerServiceError::Corrupt(error.to_string()),
        other => TriggerServiceError::Corrupt(format!(
            "placement statistics returned an unexpected failure: {other}"
        )),
    }
}

fn reconcile_statistics(
    trigger: &Trigger,
    stored: Vec<LaunchPlacementStats>,
) -> Result<TriggerStatistics, TriggerServiceError> {
    let mut placements = BTreeMap::new();
    for placement in &trigger.placements {
        if placements
            .insert(
                placement.id.clone(),
                LaunchPlacementStats {
                    placement_id: Some(placement.id.clone()),
                    launches_total: 0,
                    agents_ready_total: 0,
                },
            )
            .is_some()
        {
            return Err(TriggerServiceError::Corrupt(
                "Trigger contains duplicate placement identifiers".into(),
            ));
        }
    }

    let mut seen = BTreeSet::new();
    let mut unattributed = None;
    for row in stored {
        if row.agents_ready_total > row.launches_total {
            return Err(TriggerServiceError::Corrupt(
                "placement ready-Agent count exceeds its Launch count".into(),
            ));
        }
        match &row.placement_id {
            Some(id) => {
                if !seen.insert(id.clone()) {
                    return Err(TriggerServiceError::Corrupt(
                        "Launch statistics contain a duplicate placement row".into(),
                    ));
                }
                let slot = placements.get_mut(id).ok_or_else(|| {
                    TriggerServiceError::Corrupt(format!(
                        "Launch statistics reference unknown placement {id}"
                    ))
                })?;
                *slot = row;
            }
            None => {
                if unattributed.replace(row).is_some() {
                    return Err(TriggerServiceError::Corrupt(
                        "Launch statistics contain duplicate unattributed rows".into(),
                    ));
                }
            }
        }
    }

    let mut rows = trigger
        .placements
        .iter()
        .filter_map(|placement| placements.remove(&placement.id))
        .collect::<Vec<_>>();
    rows.extend(unattributed);
    let totals = rows.iter().try_fold(
        TriggerStats::default(),
        |mut total, row| -> Result<TriggerStats, TriggerServiceError> {
            total.launches_total = total
                .launches_total
                .checked_add(row.launches_total)
                .ok_or_else(|| TriggerServiceError::Corrupt("Launch count overflow".into()))?;
            total.agents_ready_total = total
                .agents_ready_total
                .checked_add(row.agents_ready_total)
                .ok_or_else(|| TriggerServiceError::Corrupt("ready-Agent count overflow".into()))?;
            Ok(total)
        },
    )?;
    if totals != trigger.stats {
        return Err(TriggerServiceError::Corrupt(
            "placement statistics do not match the Trigger counters".into(),
        ));
    }
    Ok(TriggerStatistics {
        totals: trigger.stats.clone(),
        placements: rows,
    })
}
