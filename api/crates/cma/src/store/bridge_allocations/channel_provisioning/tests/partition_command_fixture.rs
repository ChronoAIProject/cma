use super::*;
pub(super) struct Adapter {
    pub svc: BridgeService,
    pub bridges: Arc<MongoBridgeStore>,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub fail_preflight: AtomicBool,
    pub lose_response: AtomicBool,
    pub preflights: AtomicUsize,
}
pub(super) fn adapter(f: &ReplyFixture) -> Arc<Adapter> {
    Arc::new(Adapter {
        svc: f.svc.clone(),
        bridges: f.bridges.clone(),
        allocations: f.allocations.clone(),
        fail_preflight: AtomicBool::new(false),
        lose_response: AtomicBool::new(false),
        preflights: AtomicUsize::new(0),
    })
}
#[async_trait::async_trait]
impl BridgePartitionCommandStore for Adapter {
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
    ) -> Result<Option<PartitionCommandReceipt>, PartitionCommandError> {
        self.allocations
            .recover_partition_command(manager, id, digest)
            .await
    }
    async fn snapshot(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<PartitionCommandSnapshot, PartitionCommandError> {
        self.allocations
            .partition_command_snapshot(manager, id)
            .await
    }
    async fn apply(
        &self,
        write: PartitionCommandWrite,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError> {
        let reservation = if write.request().action == PartitionCommandAction::Reset {
            if self.fail_preflight.load(Ordering::SeqCst) {
                return Err(PartitionCommandError::Unavailable);
            }
            self.preflights.fetch_add(1, Ordering::SeqCst);
            let authority = proposal(
                &self.svc,
                &self.bridges,
                &write.management().record().id,
                "12345",
                Some(&write.snapshot().allocation),
            )
            .await
            .1;
            Some(write.reset(authority, now())?)
        } else {
            None
        };
        let receipt = self
            .allocations
            .apply_partition_command(&write, reservation.as_ref())
            .await?;
        if self.lose_response.swap(false, Ordering::SeqCst) {
            return Err(PartitionCommandError::Unavailable);
        }
        Ok(receipt)
    }
}
pub(super) async fn request(
    f: &ReplyFixture,
    allocation: &AllocationRecord,
    action: PartitionCommandAction,
) -> PartitionCommandRequest {
    PartitionCommandRequest {
        allocation_id: allocation.id.clone(),
        action,
        expected_revision: allocation.revision,
        expected_bridge_revision: f
            .bridges
            .bridge(&allocation.pin.bridge_id)
            .await
            .unwrap()
            .unwrap()
            .revision,
    }
}
/// Existing provisioning fixtures reserve with this inert source without writing
/// a callback. Supply that exact original evidence for management tests.
pub(super) async fn seed_original(f: &ReplyFixture) {
    let mut inbox = f.inbox.clone();
    inbox.id = f.allocation.first_source.inbox_id().into();
    inbox.attestation.body_sha256 = f.allocation.first_source.body_sha256().into();
    inbox.pin = f.allocation.pin.clone();
    inbox.expires_at = now() / 1000 + 3600;
    let mut caps = serde_json::to_value(inbox.capabilities.as_ref().unwrap()).unwrap();
    caps["source"] = serde_json::to_value(&f.allocation.first_source).unwrap();
    inbox.capabilities = Some(serde_json::from_value(caps).unwrap());
    let source = PartitionSourcePin::for_inbox(&inbox).unwrap();
    assert_eq!(source, f.allocation.first_source);
    let row = doc! {"_id":&inbox.id,"document_schema_version":crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
    "bridge_id":inbox.pin.bridge_id.as_str(),"admission":"accepted","expires_at":mongodb::bson::DateTime::from_millis(inbox.expires_at*1000),
    "record":crate::store::sandbox_mongo::record_bson(&inbox).unwrap()};
    f.client
        .database(&f.database)
        .collection::<Document>("bridge_callback_inbox")
        .insert_one(row)
        .await
        .unwrap();
}
