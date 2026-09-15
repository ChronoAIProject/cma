//! Source-control consent is separate from invoke grants and Message receipts.
//! Every policy revision retains its actual manager, audit and keyed result.
use super::*;
mod read;
mod records;
mod reference;
mod write;

struct Collections {
    policies: Collection<Document>,
    receipts: Collection<Document>,
    audits: Collection<Document>,
}
impl Collections {
    fn new(store: &MongoBridgeInvokeStore) -> Self {
        let database = store.bridges.client.database(&store.grants.namespace().db);
        Self {
            policies: majority_collection(&database, "bridge_source_control_policies"),
            receipts: majority_collection(&database, "bridge_source_control_receipts"),
            audits: majority_collection(&database, "bridge_source_control_audits"),
        }
    }
}
pub(super) async fn initialize(
    store: &MongoBridgeInvokeStore,
) -> Result<(), layer2_core::StoreError> {
    let database = store.bridges.client.database(&store.grants.namespace().db);
    crate::store::schema::initialize_collections(
        &database,
        &[
            "bridge_source_control_policies",
            "bridge_source_control_receipts",
            "bridge_source_control_audits",
        ],
    )
    .await?;
    Collections::new(store)
        .audits
        .create_index(
            IndexModel::builder()
                .keys(doc! {"grant_id":1_i32,"revision":1_i32})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("create source control history index".into())
        })?;
    Collections::new(store)
        .receipts
        .create_index(IndexModel::builder().keys(doc! {"grant_id":1_i32}).build())
        .await
        .map_err(|_| {
            layer2_core::StoreError::Unavailable("create source control receipt index".into())
        })?;
    Ok(())
}

#[async_trait::async_trait]
impl BridgeSourceControlStore for MongoBridgeInvokeStore {
    async fn source_control_policy(
        &self,
        access: &SourceControlAccess,
    ) -> Result<Option<SourceControlPolicy>, BridgeInvokeError> {
        Ok(self.read_source_controls(access, None).await?.0)
    }
    async fn recover_source_control(
        &self,
        access: &SourceControlAccess,
        receipt_id: &str,
        request_digest: &str,
    ) -> Result<Option<SourceControlReceipt>, BridgeInvokeError> {
        Ok(self
            .read_source_controls(access, Some((receipt_id, request_digest)))
            .await?
            .1)
    }
    async fn apply_source_control(
        &self,
        request: SourceControlWrite,
    ) -> Result<SourceControlReceipt, BridgeInvokeError> {
        self.write_source_controls(request).await
    }
}

impl MongoBridgeInvokeStore {
    async fn source_control_access(
        &self,
        access: &SourceControlAccess,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        access.require_current()?;
        let bridge = self
            .read_bridge(&access.management().record().id, session)
            .await?;
        if bridge != *access.management().record() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let current = self
            .grants
            .find_one(doc! {"_id":access.grant().id.as_str()})
            .session(&mut *session)
            .await?
            .as_ref()
            .map(read_grant)
            .transpose()?
            .ok_or(BridgeInvokeError::NotFound)?;
        if current != *access.grant() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let target =
            read_execution_snapshot(&self.resources, &current.target.resource(), session).await?;
        if target != *access.target() {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        access.require_current()?;
        Ok(())
    }
    async fn source_control_fence(
        &self,
        access: &SourceControlAccess,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.source_control_access(access, session).await?;
        fence_lineage(&self.resources, access.target(), session).await?;
        self.fence_bridge(access.management().record(), session)
            .await?;
        let changed = self
            .grants
            .update_one(
                doc! {"_id":access.grant().id.as_str(),"revision":access.grant().revision as i64},
                doc! {"$inc":{"source_control_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        access.require_current()?;
        Ok(())
    }
}
