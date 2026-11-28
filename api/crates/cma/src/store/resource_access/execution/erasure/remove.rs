use super::*;
use mongodb::{Collection, Database};
use std::collections::{BTreeMap, BTreeSet};

const MAX_FAMILY_ROWS: i64 = 256;

pub(in crate::store) struct Scope<'a> {
    pub subject: &'a str,
    pub sandboxes: &'a BTreeSet<String>,
    pub organization_sandboxes: &'a BTreeSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub(in crate::store) enum EraseError {
    #[error("execution authority still requires the records")]
    Retained,
    #[error("execution erasure evidence is inconsistent")]
    Corrupt,
    #[error("execution erasure cohort exceeds its transaction limit")]
    Capacity,
    #[error("execution erasure transaction outcome is unresolved")]
    Unresolved,
    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),
}

type Deleted = BTreeMap<&'static str, u64>;

pub(in crate::store) async fn erase(
    db: &Database,
    collection: &'static str,
    id: &str,
    scope: &Scope<'_>,
) -> Result<Deleted, EraseError> {
    let collections = Collections::from_database(db);
    let mut session = db.client().start_session().await?;
    start_transaction(&mut session).await?;
    let plan = Removal {
        collections: &collections,
        scope,
    };
    let deleted = match plan.remove(collection, id, &mut session).await {
        Ok(deleted) => deleted,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(error);
        }
    };
    if !matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ) {
        return Err(EraseError::Unresolved);
    }
    Ok(deleted)
}

struct Removal<'a> {
    collections: &'a Collections,
    scope: &'a Scope<'a>,
}

impl Removal<'_> {
    async fn remove(
        &self,
        name: &str,
        id: &str,
        session: &mut ClientSession,
    ) -> Result<Deleted, EraseError> {
        let collections = self.collections;
        let collection = match name {
            MEMBERS => &collections.members,
            EFFECTS => &collections.effects,
            GRANTS => &collections.grants,
            OPERATIONS => &collections.operations,
            _ => return Err(EraseError::Corrupt),
        };
        let Some(row) = find_row(collection, id, session).await? else {
            return Ok(Deleted::new());
        };
        let keys = read_attribution(&row)?;
        if !self.owns(&keys) && !self.actor(&keys) {
            return Err(EraseError::Retained);
        }
        match name {
            MEMBERS => self.remove_member(&row, &keys, session).await,
            EFFECTS => {
                let effect = records::read_effect(&row).map_err(invalid)?;
                let parent = find_row(
                    &collections.members,
                    effect.permission.intent.registration.as_str(),
                    session,
                )
                .await?
                .ok_or(EraseError::Corrupt)?;
                if parent.get(ERASURE_KEYS) != row.get(ERASURE_KEYS) {
                    return Err(EraseError::Corrupt);
                }
                self.remove_member(&parent, &keys, session).await
            }
            OPERATIONS => {
                let operation = records::read_operation(&row).map_err(invalid)?;
                self.remove_family(&operation.key.family.origin, &keys, session)
                    .await
            }
            GRANTS => {
                let origin = id.strip_prefix("grant_receipt:").unwrap_or(id);
                let origin = wire::Digest::new(origin).map_err(|_| EraseError::Corrupt)?;
                self.remove_family(&origin, &keys, session).await
            }
            _ => Err(EraseError::Corrupt),
        }
    }

    fn owns(&self, keys: &records::ErasureKeys) -> bool {
        self.scope.sandboxes.contains(keys.sandbox_id.as_str())
            && keys
                .owner_subject
                .as_ref()
                .is_some_and(|owner| owner.as_str() == self.scope.subject)
    }

    fn actor(&self, keys: &records::ErasureKeys) -> bool {
        keys.owner_subject.is_some()
            && keys
                .actor_subject
                .as_ref()
                .is_some_and(|actor| actor.as_str() == self.scope.subject)
            && !self
                .scope
                .organization_sandboxes
                .contains(keys.sandbox_id.as_str())
    }

    async fn remove_member(
        &self,
        row: &Document,
        keys: &records::ErasureKeys,
        session: &mut ClientSession,
    ) -> Result<Deleted, EraseError> {
        let member = records::read_member(row).map_err(invalid)?;
        let owned = self.owns(keys);
        match member.basis.family() {
            Some(family) if owned => {
                return self.remove_family(&family.origin, keys, session).await
            }
            // Foreign independent executions lack a revocation generation.
            None if !owned && member.permit.is_some() => return Err(EraseError::Retained),
            _ => {}
        }
        self.require_settled_effects(&member, row, session).await?;
        if member.basis.family().is_some() {
            self.record_erased_member(&member, session).await?;
        }
        let mut deleted = Deleted::new();
        self.remove_members(
            vec![member.registration.as_str().to_owned()],
            u64::from(member.effect_count),
            &mut deleted,
            session,
        )
        .await?;
        Ok(deleted)
    }

    async fn require_settled_effects(
        &self,
        member: &ExecutionRecordV1,
        member_row: &Document,
        session: &mut ClientSession,
    ) -> Result<(), EraseError> {
        let mut effects = self
            .collections
            .effects
            .find(doc! {"registration":member.registration.as_str()})
            .sort(doc! {"ordinal":1})
            .limit(i64::from(member.effect_count) + 1)
            .session(&mut *session)
            .await?;
        let mut ordinal = 0;
        while let Some(row) = effects.next(&mut *session).await.transpose()? {
            ordinal += 1;
            let effect = records::read_effect(&row).map_err(invalid)?;
            let intent = &effect.permission.intent;
            if ordinal > member.effect_count
                || effect.permission.ledger_sequence.get() != u64::from(ordinal)
                || intent.registration != member.registration
                || intent.grant != member.basis
                || (wire::MemberKey::Execution {
                    execution: intent.execution.clone(),
                }) != member.member
                || row.get(ERASURE_KEYS) != member_row.get(ERASURE_KEYS)
            {
                return Err(EraseError::Corrupt);
            }
            if !effect.observations.last().is_some_and(|observation| {
                matches!(
                    observation.outcome,
                    wire::EffectOutcome::CommitConfirmed | wire::EffectOutcome::Rejected
                )
            }) {
                return Err(EraseError::Retained);
            }
        }
        if ordinal != member.effect_count {
            return Err(EraseError::Corrupt);
        }
        Ok(())
    }

    async fn record_erased_member(
        &self,
        member: &ExecutionRecordV1,
        session: &mut ClientSession,
    ) -> Result<(), EraseError> {
        let id = &member
            .barrier
            .as_ref()
            .ok_or(EraseError::Retained)?
            .request
            .revocation
            .operation_id;
        let raw = find_row(&self.collections.operations, id.as_str(), session)
            .await?
            .ok_or(EraseError::Corrupt)?;
        let mut operation = records::read_operation(&raw).map_err(invalid)?;
        if !matches!(operation.phase, RevocationPhaseV1::Effective { .. }) {
            return Err(EraseError::Retained);
        }
        let request = member.barrier_request(&operation).map_err(invalid)?;
        if member.completion.is_none()
            || member.epoch != operation.epoch
            || member.barrier.as_ref().map(|barrier| &barrier.request) != Some(&request)
            || !operation.erased_ordinals.insert(member.ordinal)
        {
            return Err(EraseError::Corrupt);
        }
        let updated = self
            .collections
            .operations
            .update_one(
                doc! {"_id":id.as_str(),"phase":"effective"},
                records::update_record(records::operation_document(&operation).map_err(invalid)?),
            )
            .session(&mut *session)
            .await?;
        if updated.matched_count != 1 {
            return Err(EraseError::Corrupt);
        }
        Ok(())
    }

    async fn remove_family(
        &self,
        origin: &wire::Digest,
        selected: &records::ErasureKeys,
        session: &mut ClientSession,
    ) -> Result<Deleted, EraseError> {
        let collections = self.collections;
        let receipt_id = records::grant_receipt_id(origin);
        let raw = find_row(&collections.grants, origin.as_str(), session).await?;
        let receipt = find_row(&collections.grants, &receipt_id, session).await?;
        let control = records::grant_pair(origin, raw.as_ref(), receipt.as_ref())
            .map_err(invalid)?
            .ok_or(EraseError::Corrupt)?;
        let keys = read_attribution(&raw.ok_or(EraseError::Corrupt)?)?;
        if !self.owns(&keys) || !same_scope(selected, &keys) {
            return Err(EraseError::Retained);
        }
        if matches!(control.state, GrantStateV1::Closing { .. }) {
            return Err(EraseError::Retained);
        }
        // Only keyed controls are eligible. Their writers always index all
        // subsequent revocation operations, even when admission is disabled.
        let operations = self.family_operations(&control, &keys, session).await?;
        let (registrations, effect_count) = self
            .family_members(&control, &keys, &operations, session)
            .await?;
        let mut deleted = Deleted::new();
        self.remove_members(registrations, effect_count, &mut deleted, session)
            .await?;
        remove_ids(&collections.operations, operations, &mut deleted, session).await?;
        remove_ids(
            &collections.grants,
            vec![origin.as_str().to_owned(), receipt_id],
            &mut deleted,
            session,
        )
        .await?;
        Ok(deleted)
    }

    async fn family_members(
        &self,
        control: &GrantControlV1,
        keys: &records::ErasureKeys,
        operations: &[String],
        session: &mut ClientSession,
    ) -> Result<(Vec<String>, u64), EraseError> {
        let rows = &self.collections.members;
        let mut cursor = rows
            .find(doc! {"family_origin": control.key.origin.as_str()})
            .limit(MAX_FAMILY_ROWS + 1)
            .session(&mut *session)
            .await?;
        let mut ids = Vec::new();
        let mut effect_count = 0;
        let mut current_members = 0;
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            if ids.len() == MAX_FAMILY_ROWS as usize {
                return Err(EraseError::Capacity);
            }
            let member = records::read_member(&row).map_err(invalid)?;
            let family = member.basis.family().ok_or(EraseError::Corrupt)?;
            if family.origin != control.key.origin
                || family.share_id != control.key.share_id
                || family.generation > control.key.generation
                || !same_scope(&read_attribution(&row)?, keys)
                || member.barrier.as_ref().is_some_and(|barrier| {
                    !operations
                        .iter()
                        .any(|id| id == barrier.request.revocation.operation_id.as_str())
                })
            {
                return Err(EraseError::Corrupt);
            }
            self.require_settled_effects(&member, &row, session).await?;
            effect_count += u64::from(member.effect_count);
            if family.generation == control.key.generation {
                current_members += 1;
            }
            ids.push(member.registration.as_str().to_owned());
        }
        if current_members > control.members
            || matches!(control.state, GrantStateV1::Open) && current_members != control.members
        {
            return Err(EraseError::Corrupt);
        }
        Ok((ids, effect_count))
    }

    async fn family_operations(
        &self,
        control: &GrantControlV1,
        keys: &records::ErasureKeys,
        session: &mut ClientSession,
    ) -> Result<Vec<String>, EraseError> {
        let rows = &self.collections.operations;
        let mut cursor = rows
            .find(doc! {SHARE:control.key.share_id.as_str()})
            .limit(MAX_FAMILY_ROWS + 1)
            .session(&mut *session)
            .await?;
        let mut ids = Vec::new();
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            if ids.len() == MAX_FAMILY_ROWS as usize {
                return Err(EraseError::Capacity);
            }
            let operation = records::read_operation(&row).map_err(invalid)?;
            if operation.key.family.origin != control.key.origin
                || !same_scope(&read_attribution(&row)?, keys)
            {
                return Err(EraseError::Corrupt);
            }
            if !matches!(operation.phase, RevocationPhaseV1::Effective { .. }) {
                return Err(EraseError::Retained);
            }
            ids.push(operation.key.operation_id.as_str().to_owned());
        }
        if let GrantStateV1::Closed { operation } = &control.state {
            if !ids.iter().any(|id| id == operation.as_str()) {
                return Err(EraseError::Corrupt);
            }
        }
        Ok(ids)
    }

    async fn remove_members(
        &self,
        ids: Vec<String>,
        expected_effects: u64,
        deleted: &mut Deleted,
        session: &mut ClientSession,
    ) -> Result<(), EraseError> {
        if !ids.is_empty() {
            let count = self
                .collections
                .effects
                .delete_many(doc! {"registration":{"$in": &ids}})
                .session(&mut *session)
                .await?
                .deleted_count;
            if count != expected_effects {
                return Err(EraseError::Corrupt);
            }
            if count > 0 {
                deleted.insert(EFFECTS, count);
            }
        }
        remove_ids(&self.collections.members, ids, deleted, session).await
    }
}

async fn find_row(
    collection: &Collection<Document>,
    id: &str,
    session: &mut ClientSession,
) -> Result<Option<Document>, EraseError> {
    Ok(collection
        .find_one(doc! {"_id":id})
        .session(session)
        .await?)
}

async fn remove_ids(
    collection: &Collection<Document>,
    ids: Vec<String>,
    deleted: &mut Deleted,
    session: &mut ClientSession,
) -> Result<(), EraseError> {
    if ids.is_empty() {
        return Ok(());
    }
    let count = collection
        .delete_many(doc! {"_id":{"$in": &ids}})
        .session(session)
        .await?
        .deleted_count;
    if count != ids.len() as u64 {
        return Err(EraseError::Corrupt);
    }
    let name = COLLECTIONS
        .iter()
        .copied()
        .find(|name| *name == collection.name())
        .ok_or(EraseError::Corrupt)?;
    deleted.insert(name, count);
    Ok(())
}

fn read_attribution(row: &Document) -> Result<records::ErasureKeys, EraseError> {
    let document = row
        .get_document(ERASURE_KEYS)
        .map_err(|_| EraseError::Retained)?;
    mongodb::bson::deserialize_from_document(document.clone()).map_err(|_| EraseError::Corrupt)
}

fn same_scope(left: &records::ErasureKeys, right: &records::ErasureKeys) -> bool {
    left.sandbox_id == right.sandbox_id
        && left.share_id == right.share_id
        && left.owner_subject == right.owner_subject
}

fn invalid(_: ExecutionError) -> EraseError {
    EraseError::Corrupt
}
