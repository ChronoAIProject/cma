use super::*;
use futures_util::TryStreamExt;
use mongodb::{options::IndexOptions, Collection, IndexModel};

pub(in crate::store) const GRANTS: &str = "resource_execution_grants_v1";
pub(in crate::store) const MEMBERS: &str = "resource_execution_records_v1";
pub(in crate::store) const OPERATIONS: &str = "resource_revocation_operations_v1";
pub(in crate::store) const EFFECTS: &str = "resource_execution_effects_v1";
const ACTIVATION: &str = "activation";
const ACTIVATION_RECEIPT: &str = "activation_receipt";

#[derive(Clone)]
pub(super) struct Collections {
    pub grants: Collection<Document>,
    pub members: Collection<Document>,
    pub operations: Collection<Document>,
    pub effects: Collection<Document>,
}

impl Collections {
    pub fn new(sandbox: &MongoSandboxStore) -> Self {
        let database = sandbox
            .client
            .database(&sandbox.resource_shares.namespace().db);
        Self::from_database(&database)
    }

    pub(super) fn from_database(database: &mongodb::Database) -> Self {
        use crate::store::durability::majority_collection;
        Self {
            grants: majority_collection(database, GRANTS),
            members: majority_collection(database, MEMBERS),
            operations: majority_collection(database, OPERATIONS),
            effects: majority_collection(database, EFFECTS),
        }
    }

    pub async fn grant(&self, origin: &wire::Digest) -> ExecutionResult<Option<GrantControlV1>> {
        let receipt_id = records::grant_receipt_id(origin);
        let documents: Vec<_> = self
            .grants
            .find(doc! {"_id":{"$in":[origin.as_str(), &receipt_id]}})
            .limit(2)
            .batch_size(2)
            .await
            .map_err(|_| ExecutionError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let control = documents
            .iter()
            .find(|row| row.get_str("_id").is_ok_and(|id| id == origin.as_str()));
        let receipt = documents
            .iter()
            .find(|row| row.get_str("_id").is_ok_and(|id| id == receipt_id));
        records::grant_pair(origin, control, receipt)
    }

    pub async fn grant_in(
        &self,
        origin: &wire::Digest,
        session: &mut ClientSession,
    ) -> TxnResult<Option<GrantControlV1>> {
        let control = self
            .grants
            .find_one(doc! {"_id":origin.as_str()})
            .session(&mut *session)
            .await?;
        let receipt = self
            .grants
            .find_one(doc! {"_id":records::grant_receipt_id(origin)})
            .session(session)
            .await?;
        Ok(records::grant_pair(
            origin,
            control.as_ref(),
            receipt.as_ref(),
        )?)
    }

    pub async fn ensure_indexes(&self) -> ExecutionResult<()> {
        let members = IndexModel::builder()
            .keys(doc! {"family_origin":1,"generation":1,"ordinal":1})
            .options(
                IndexOptions::builder()
                    .name("frozen_member_order_v1".to_owned())
                    .unique(true)
                    .partial_filter_expression(doc! {"family_origin":{"$type":"string"}})
                    .build(),
            )
            .build();
        self.members
            .create_index(members)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let effects = IndexModel::builder()
            .keys(doc! {"registration":1,"ordinal":1})
            .options(
                IndexOptions::builder()
                    .name("effect_ordinal_v1".to_owned())
                    .unique(true)
                    .build(),
            )
            .build();
        self.effects
            .create_index(effects)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let pending = IndexModel::builder().keys(doc! {"phase":1,"_id":1}).build();
        self.operations
            .create_index(pending)
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        Ok(())
    }

    pub async fn activation(&self) -> ExecutionResult<ActivationStateV1> {
        let document = self
            .grants
            .find_one(doc! {"_id":ACTIVATION})
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        let receipt = self
            .grants
            .find_one(doc! {"_id":ACTIVATION_RECEIPT})
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        activation_pair(document.as_ref(), receipt.as_ref())?.ok_or(ExecutionError::Corrupt)
    }

    async fn activation_in(
        &self,
        session: &mut ClientSession,
    ) -> TxnResult<Option<ActivationStateV1>> {
        let document = self
            .grants
            .find_one(doc! {"_id":ACTIVATION})
            .session(&mut *session)
            .await?;
        let receipt = self
            .grants
            .find_one(doc! {"_id":ACTIVATION_RECEIPT})
            .session(session)
            .await?;
        Ok(activation_pair(document.as_ref(), receipt.as_ref())?)
    }

    pub async fn require_activation(
        &self,
        session: &mut ClientSession,
    ) -> TxnResult<wire::Generation> {
        let state = self
            .activation_in(session)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        match state {
            ActivationStateV1::Activated { epoch } => Ok(epoch),
            ActivationStateV1::NeverActivated => Err(ExecutionError::Disabled.into()),
        }
    }

    pub async fn initialize(&self, session: &mut ClientSession) -> TxnResult<()> {
        if self.activation_in(session).await?.is_none() {
            self.grants
                .insert_one(records::encode(
                    ACTIVATION,
                    &ActivationStateV1::NeverActivated,
                )?)
                .session(session)
                .await?;
        }
        Ok(())
    }

    pub async fn activate(
        &self,
        epoch: wire::Generation,
        evidence: &wire::Digest,
        session: &mut ClientSession,
    ) -> TxnResult<()> {
        let state = self
            .activation_in(session)
            .await?
            .ok_or(ExecutionError::Corrupt)?;
        let mut active = records::encode(ACTIVATION, &ActivationStateV1::Activated { epoch })?;
        active.insert("drain_evidence", evidence.as_str());
        let mut receipt = active.clone();
        receipt.insert("_id", ACTIVATION_RECEIPT);
        if matches!(state, ActivationStateV1::Activated { .. }) {
            let retained = self
                .grants
                .find_one(doc! {"_id":ACTIVATION_RECEIPT})
                .session(session)
                .await?;
            return if retained.as_ref() == Some(&receipt) {
                Ok(())
            } else {
                Err(ExecutionError::IdempotencyConflict.into())
            };
        }
        let result = self
            .grants
            .replace_one(
                records::encode(ACTIVATION, &ActivationStateV1::NeverActivated)?,
                active,
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(ExecutionError::AuthorityChanged.into());
        }
        // This immutable receipt survives loss of the primary latch. Both
        // writes commit together; startup never infers legacy from one missing.
        self.grants.insert_one(receipt).session(session).await?;
        Ok(())
    }
}

pub(super) fn activation_pair(
    latch: Option<&Document>,
    receipt: Option<&Document>,
) -> ExecutionResult<Option<ActivationStateV1>> {
    let Some(latch) = latch else {
        return if receipt.is_none() {
            Ok(None)
        } else {
            Err(ExecutionError::Corrupt)
        };
    };
    let state = read_activation(latch)?;
    match (&state, receipt) {
        (ActivationStateV1::NeverActivated, None) => Ok(Some(state)),
        (ActivationStateV1::Activated { .. }, Some(receipt)) => {
            let mut expected = latch.clone();
            expected.insert("_id", ACTIVATION_RECEIPT);
            if &expected != receipt {
                return Err(ExecutionError::Corrupt);
            }
            Ok(Some(state))
        }
        _ => Err(ExecutionError::Corrupt),
    }
}

fn read_activation(document: &Document) -> ExecutionResult<ActivationStateV1> {
    let state: ActivationStateV1 = records::decode(document)?;
    let mut expected = records::encode(ACTIVATION, &state)?;
    if matches!(state, ActivationStateV1::Activated { .. }) {
        let evidence = document
            .get_str("drain_evidence")
            .map_err(|_| ExecutionError::Corrupt)?;
        wire::Digest::new(evidence).map_err(|_| ExecutionError::Corrupt)?;
        expected.insert("drain_evidence", evidence);
    }
    if expected != *document {
        return Err(ExecutionError::Corrupt);
    }
    Ok(state)
}

pub(in crate::store::resource_access) async fn refuse_legacy_change(
    sandbox: &MongoSandboxStore,
    request: &layer2_core::resource_access::ShareMutation,
    session: &mut ClientSession,
) -> Result<(), super::super::ReadError> {
    use layer2_core::resource_access::{ResourceAccessStoreError as E, ShareChange};
    if !matches!(
        request.change,
        ShareChange::SetMode { .. } | ShareChange::Revoke { .. } | ShareChange::RedeemLink { .. }
    ) {
        return Ok(());
    }
    let collections = Collections::new(sandbox);
    let state = collections
        .activation_in(session)
        .await
        .map_err(|error| match error {
            TxnError::Domain(_) => super::super::ReadError::Domain(E::Corrupt),
            TxnError::Driver(error) => super::super::ReadError::Driver(error),
        })?;
    if let Some(ActivationStateV1::Activated { epoch }) = state {
        let ShareChange::RedeemLink { share_id, .. } = &request.change else {
            return Err(E::Conflict.into());
        };
        // Redemption advances the same share revision that finalization pins.
        // The existing lineage write makes this read conflict with freeze.
        let snapshot = super::super::snapshot::read(sandbox, &request.target(), session).await?;
        let share = snapshot
            .shares
            .iter()
            .find(|share| &share.id == share_id)
            .ok_or(E::NotFound)?;
        let origin = origin_digest(&snapshot, share).map_err(|_| E::Corrupt)?;
        if let Some(control) = collections
            .grant_in(&origin, session)
            .await
            .map_err(|error| match error {
                TxnError::Domain(_) => super::super::ReadError::Domain(E::Corrupt),
                TxnError::Driver(error) => super::super::ReadError::Driver(error),
            })?
        {
            if control.key.share_id.as_str() != share_id || control.epoch != epoch {
                return Err(E::Corrupt.into());
            }
            if !matches!(control.state, GrantStateV1::Open) {
                return Err(E::Conflict.into());
            }
        }
    }
    Ok(())
}

impl MongoSandboxStore {
    /// Explicit one-time bootstrap after deployment provenance is verified.
    /// Composition must never call this to recover a missing activation latch.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 owns activation composition; R1 exposes the inactive store port"
        )
    )]
    pub async fn initialize_resource_execution_once(&self) -> ExecutionResult<()> {
        let collections = Collections::new(self);
        collections.ensure_indexes().await?;
        let store = MongoResourceExecutionStore {
            sandbox: self.clone(),
            collections,
            write_erasure_keys: false,
        };
        match store.transact(Command::Bootstrap).await? {
            Output::Observed => Ok(()),
            _ => Err(ExecutionError::Corrupt),
        }
    }

    /// R3 supplies the reviewed legacy-drain/capability evidence. This method
    /// cannot turn activation off or replace an established policy epoch.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 owns activation composition; R1 exposes the inactive store port"
        )
    )]
    pub async fn activate_resource_execution_after_drain(
        &self,
        epoch: wire::Generation,
        drain_evidence: &wire::Digest,
    ) -> ExecutionResult<()> {
        let collections = Collections::new(self);
        let store = MongoResourceExecutionStore {
            sandbox: self.clone(),
            collections,
            write_erasure_keys: false,
        };
        match store
            .transact(Command::Activate(epoch, drain_evidence))
            .await?
        {
            Output::Observed => Ok(()),
            _ => Err(ExecutionError::Corrupt),
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 owns activation composition; R1 exposes the inactive store port"
        )
    )]
    /// The erasure-key writer may be selected only after all readers are
    /// compatible and older replacement writers have drained.
    pub async fn open_resource_execution(
        &self,
        write_erasure_keys: bool,
    ) -> ExecutionResult<Arc<dyn ResourceExecutionStore>> {
        let collections = Collections::new(self);
        if !matches!(
            collections.activation().await?,
            ActivationStateV1::Activated { .. }
        ) {
            return Err(ExecutionError::Disabled);
        }
        collections.ensure_indexes().await?;
        Ok(Arc::new(MongoResourceExecutionStore {
            sandbox: self.clone(),
            collections,
            write_erasure_keys,
        }))
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "R3 owns activation composition; R1 exposes the inactive store port"
        )
    )]
    pub async fn resource_execution_activation(&self) -> ExecutionResult<ActivationStateV1> {
        Collections::new(self).activation().await
    }
}

#[cfg(test)]
mod isolation_role_tests {
    // Share mode changes, revocations and link redemption read the activation
    // record, so an isolated runtime role must grant every execution store.
    #[test]
    fn isolated_runtime_role_grants_every_execution_collection() {
        let role = crate::store::isolation::role("matrix");
        let granted: Vec<String> = role
            .get_array("privileges")
            .unwrap()
            .iter()
            .filter_map(|grant| {
                grant
                    .as_document()?
                    .get_document("resource")
                    .ok()?
                    .get_str("collection")
                    .ok()
                    .map(str::to_owned)
            })
            .collect();
        for collection in [
            super::GRANTS,
            super::MEMBERS,
            super::OPERATIONS,
            super::EFFECTS,
        ] {
            assert!(
                granted.iter().any(|name| name == collection),
                "{collection} is missing from the isolated runtime role"
            );
            assert!(crate::store::isolation::OPTIONAL_COLLECTIONS.contains(&collection));
        }
    }
}
