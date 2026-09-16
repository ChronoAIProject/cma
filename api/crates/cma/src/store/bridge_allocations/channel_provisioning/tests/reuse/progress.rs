use super::*;
use crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase;

impl ReuseFixture {
    pub async fn reserve(&self, sender: &str) -> AllocationRecord {
        let (admission, proof) = self.proposal(sender).await;
        self.original
            .allocations
            .reserve(AllocationReservation::new(admission, proof, now()).unwrap())
            .await
            .unwrap()
    }

    pub async fn reset(
        &self,
        record: &AllocationRecord,
        sender: &str,
    ) -> Result<AllocationRecord, AllocationError> {
        let (admission, _) = self.proposal(sender).await;
        let admission = admission.after_reset(record)?;
        let proof = self.proof(admission.identity(), None).await;
        let bridge = self
            .original
            .bridges
            .bridge(&record.identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let management = self
            .original
            .svc
            .management_lease(&caller(), None, &bridge.id, bridge.revision)
            .await
            .unwrap();
        let request =
            AllocationReservation::reset(admission, proof, record.clone(), management, now())?;
        self.original.allocations.reserve(request).await
    }

    pub async fn current(&self, record: &AllocationRecord) -> AllocationRecord {
        self.original
            .allocations
            .current(&record.identity)
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn release(&self, record: &AllocationRecord) -> AllocationRecord {
        for _ in 0..32 {
            let current = self.current(record).await;
            let Some(claim) = current.lease.clone() else {
                return current;
            };
            let proof = self.authority(&current).await;
            let change = AllocationMutation::progress(
                current,
                proof,
                &claim,
                AllocationProgress::NeedsConnection,
                now(),
            )
            .unwrap();
            match self.original.allocations.mutate(change).await {
                Ok(next) => return next,
                Err(AllocationError::Conflict | AllocationError::Unavailable) => {
                    tokio::task::yield_now().await
                }
                Err(error) => panic!("release failed: {error}"),
            }
        }
        panic!("reuse claim release did not converge");
    }

    pub async fn step(
        &self,
        record: AllocationRecord,
    ) -> Result<AllocationRecord, AllocationError> {
        let proof = self.authority(&record).await;
        self.driver()
            .run_one(record, &proof, &self.original.lease)
            .await
    }

    pub async fn ready(&self, sender: &str) -> AllocationRecord {
        self.drive_ready(self.reserve(sender).await).await
    }

    pub async fn drive_ready(&self, mut record: AllocationRecord) -> AllocationRecord {
        for _ in 0..64 {
            record = self.current(&record).await;
            if record.state == AllocationState::Ready {
                return record;
            }
            record = self.release(&record).await;
            match self.step(record.clone()).await {
                Ok(next) => record = next,
                Err(AllocationError::Conflict | AllocationError::Unavailable) => {
                    tokio::task::yield_now().await;
                }
                Err(error) => panic!("reuse startup failed: {error}"),
            }
        }
        panic!("reuse startup did not converge");
    }

    pub async fn agent_start(&self, sender: &str) -> AllocationRecord {
        let mut record = self.reserve(sender).await;
        for _ in 0..8 {
            record = self.step(record).await.unwrap();
            let row = self
                .original
                .client
                .database(&self.original.database)
                .collection::<Document>("bridge_allocations")
                .find_one(doc! {"_id":&record.id})
                .await
                .unwrap()
                .unwrap();
            let state: crate::sandbox::channel_reuse::state::ChannelReuseState =
                mongodb::bson::deserialize_from_bson(row.get("channel_reuse").unwrap().clone())
                    .unwrap();
            record = self.release(&record).await;
            if state.phase == ChannelProvisioningPhase::AgentStart {
                return record;
            }
        }
        panic!("reuse child admission did not converge");
    }
}
