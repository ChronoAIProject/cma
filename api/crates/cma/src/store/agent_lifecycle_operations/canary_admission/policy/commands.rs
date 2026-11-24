//! Issue and revoke bounded, owner-scoped runtime canary grants.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(crate) struct PrepareCanaryGrant {
    pub(crate) owner: SubjectId,
    pub(crate) expected_revision: i64,
    pub(crate) release_key: String,
    pub(crate) policy_revision: i64,
    pub(crate) consent_sha256: String,
    pub(crate) expires_at: DateTime,
    pub(crate) fresh: Option<FreshCanaryGrant>,
}

pub(crate) struct FreshCanaryGrant {
    pub(crate) lane: CanaryLane,
    pub(crate) release: FrozenRelease,
    pub(crate) catalog_sha256: String,
    pub(crate) owner_scope_sha256: String,
}

pub(crate) struct RevokeCanaryGrant {
    pub(crate) owner: SubjectId,
    pub(crate) release_key: String,
    pub(crate) grant_id: String,
    pub(crate) revision: i64,
}

const LANES: [CanaryLane; 2] = [CanaryLane::Agent, CanaryLane::DebianDesktop];

impl CanaryGrantSlot {
    fn issuance_receipt(&self, request: &PrepareCanaryGrant) -> Option<Self> {
        let revision = match self.state {
            CanaryGrantState::Issued => self.revision,
            _ => self.revision.checked_sub(1)?,
        };
        (revision == request.expected_revision.checked_add(1)?
            && self.release_key == request.release_key
            && self.policy_revision == request.policy_revision
            && self.consent_sha256 == request.consent_sha256
            && self.expires_at == request.expires_at)
            .then(|| Self {
                revision,
                state: CanaryGrantState::Issued,
                ..self.clone()
            })
    }
}

async fn find_grant(
    database: &Database,
    owner: &SubjectId,
    session: &mut ClientSession,
    matches: impl Fn(&CanaryGrantSlot) -> Option<CanaryGrantSlot> + Send,
) -> Result<Option<(CanaryLane, CanaryGrantSlot)>, CanaryWriteError> {
    let mut found = None;
    for lane in LANES {
        if let Some(receipt) = slot(database, owner, lane, session)
            .await?
            .as_ref()
            .and_then(&matches)
        {
            if found.replace((lane, receipt)).is_some() {
                return Err(CanaryWriteError::Refused);
            }
        }
    }
    Ok(found)
}

impl MongoAgentLifecycleStore {
    pub(crate) async fn issue_canary_grant(
        &self,
        database: &Database,
        requested: &PrepareCanaryGrant,
    ) -> Result<CanaryGrantSlot, CanaryWriteError> {
        if requested.expected_revision < 0 || !canonical_sha256(&requested.consent_sha256) {
            return Err(CanaryWriteError::Refused);
        }
        let mut session = database.client().start_session().await?;
        crate::store::start_transaction(&mut session).await?;
        let created = async {
            let owner = &requested.owner;
            if account_deletions::erasing_in_session(database, owner, &mut session).await? {
                return Err(CanaryWriteError::OwnerUnavailable);
            }
            if let Some((_, receipt)) = find_grant(database, owner, &mut session, |stored| {
                stored.issuance_receipt(requested)
            })
            .await?
            {
                return Ok(receipt);
            }
            let fresh = requested.fresh.as_ref().ok_or(CanaryWriteError::Closed)?;
            if fresh.release.release_key != requested.release_key {
                return Err(CanaryWriteError::Refused);
            }
            fence_owner(database, owner, &mut session).await?;
            fence_policy(
                database,
                requested.policy_revision,
                &fresh.catalog_sha256,
                &fresh.owner_scope_sha256,
                &mut session,
            )
            .await?;
            let digest = image_digest(&fresh.release.image.uri).ok_or_else(invalid_authority)?;
            let descriptor = layer2_core::profiles::content_digest(&fresh.release)
                .map_err(|_| invalid_authority())?;
            let current = slot(database, owner, fresh.lane, &mut session).await?;
            if current.as_ref().map_or(0, |grant| grant.revision) != requested.expected_revision
                || self
                    .operations
                    .find_one(doc! {"owner": owner.as_str(), MANIFEST_DIGEST: &digest})
                    .session(&mut session)
                    .await?
                    .is_some()
            {
                return Err(CanaryWriteError::Refused);
            }
            let next = CanaryGrantSlot {
                id: uuid::Uuid::new_v4().simple().to_string(),
                revision: next_revision(requested.expected_revision)?,
                release_key: requested.release_key.clone(),
                manifest_digest: digest,
                descriptor_sha256: descriptor,
                consent_sha256: requested.consent_sha256.clone(),
                expires_at: requested.expires_at,
                policy_revision: requested.policy_revision,
                state: CanaryGrantState::Issued,
            };
            replace_slot(
                database,
                owner,
                fresh.lane,
                current.as_ref(),
                &next,
                &mut session,
            )
            .await?;
            Ok(next)
        }
        .await;
        finish_grant_write(&mut session, created).await
    }

    pub(crate) async fn revoke_canary_grant(
        &self,
        database: &Database,
        request: &RevokeCanaryGrant,
    ) -> Result<CanaryGrantSlot, CanaryWriteError> {
        let mut session = database.client().start_session().await?;
        crate::store::start_transaction(&mut session).await?;
        let changed = async {
            fence_owner(database, &request.owner, &mut session).await?;
            let (lane, current) = find_grant(database, &request.owner, &mut session, |stored| {
                (stored.id == request.grant_id
                    && stored.release_key == request.release_key
                    && stored.revision == request.revision
                    && stored.state == CanaryGrantState::Issued)
                    .then(|| stored.clone())
            })
            .await?
            .ok_or(CanaryWriteError::Refused)?;
            let next = CanaryGrantSlot {
                state: CanaryGrantState::Revoked,
                revision: next_revision(current.revision)?,
                ..current.clone()
            };
            replace_slot(
                database,
                &request.owner,
                lane,
                Some(&current),
                &next,
                &mut session,
            )
            .await?;
            Ok(next)
        }
        .await;
        finish_grant_write(&mut session, changed).await
    }
}

async fn finish_grant_write(
    session: &mut ClientSession,
    mutation: Result<CanaryGrantSlot, CanaryWriteError>,
) -> Result<CanaryGrantSlot, CanaryWriteError> {
    use crate::store::{commit_transaction, CommitOutcome};
    let value = match mutation {
        Ok(value) => value,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
    };
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(value),
        CommitOutcome::Retry(_) | CommitOutcome::Unknown(_) => Err(CanaryWriteError::Unresolved),
        CommitOutcome::Failed(error) => Err(error.into()),
    }
}
