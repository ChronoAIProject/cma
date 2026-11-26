use super::*;
use mongodb::{bson::doc, Collection};
use serde::{Deserialize, Serialize};

pub(super) const COLLECTION: &str = "revocation_process_fixture";
pub(super) const CHILD: &str =
    "commands::resource_execution::tests::process_child::revocation_worker_child";
pub(super) const COMPLETED: &str = "REVOCATION_CHILD_EFFECTIVE";

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Cut {
    BeforeStop,
    AfterReceipt,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Script {
    #[serde(rename = "_id")]
    pub id: String,
    pub request: wire::BarrierRequestV1,
    pub receipt: wire::StopReceiptV1,
    pub cut: Cut,
    pub reached: bool,
    pub retained: bool,
    pub calls: Vec<String>,
    pub stop_effects: u32,
}

impl Script {
    pub fn new(row: &ExecutionRecordV1, operation: &RevocationOperationV1, cut: Cut) -> Self {
        let request = row.barrier_request(operation).unwrap();
        let directory = Directory::new(std::slice::from_ref(row));
        let MemberCompletionV1::Execution { receipt } = directory.endpoint(1).completion(&request)
        else {
            panic!()
        };
        Self {
            id: "participant".into(),
            request,
            receipt,
            cut,
            reached: false,
            retained: false,
            calls: Vec::new(),
            stop_effects: 0,
        }
    }
}

// This collection is a synthetic receiver journal, separate from product records.
pub(super) struct Receiver {
    pub script: Script,
    pub collection: Collection<Script>,
    pub pause: bool,
}

impl Receiver {
    async fn called(
        &self,
        request: &wire::BarrierRequestV1,
        action: &str,
    ) -> wire::ProtocolResult<()> {
        if request != &self.script.request {
            return Err(wire::ProtocolError::EvidenceMismatch);
        }
        self.collection
            .update_one(
                doc! {"_id": &self.script.id},
                doc! {"$push": {"calls": action}},
            )
            .await
            .map_err(|_| wire::ProtocolError::Unavailable)?;
        Ok(())
    }

    async fn stop_at_cut(&self) -> wire::ProtocolResult<()> {
        self.collection
            .update_one(
                doc! {"_id": &self.script.id},
                doc! {"$set": {"reached": true}},
            )
            .await
            .map_err(|_| wire::ProtocolError::Unavailable)?;
        std::future::pending().await
    }
}

#[async_trait::async_trait]
impl wire::RuntimeExecutionControl for Receiver {
    async fn stop_receipt(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> wire::ProtocolResult<Option<wire::StopReceiptV1>> {
        self.called(request, "query").await?;
        let script = self
            .collection
            .find_one(doc! {"_id": &self.script.id})
            .await
            .map_err(|_| wire::ProtocolError::Unavailable)?
            .ok_or(wire::ProtocolError::Unavailable)?;
        Ok(script.retained.then_some(script.receipt))
    }

    async fn install_barrier(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> wire::ProtocolResult<wire::BarrierReceiptV1> {
        self.called(request, "barrier").await?;
        Ok(self.script.receipt.barrier.clone())
    }

    async fn cancel(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> wire::ProtocolResult<wire::StopReceiptV1> {
        self.called(request, "stop").await?;
        if self.pause && matches!(self.script.cut, Cut::BeforeStop) {
            self.stop_at_cut().await?;
        }
        self.collection
            .update_one(
                doc! {"_id": &self.script.id, "retained": false},
                doc! {"$set": {"retained": true}, "$inc": {"stop_effects": 1}},
            )
            .await
            .map_err(|_| wire::ProtocolError::Unavailable)?;
        if self.pause && matches!(self.script.cut, Cut::AfterReceipt) {
            self.stop_at_cut().await?;
        }
        Ok(self.script.receipt.clone())
    }
}

pub(super) struct ProcessDirectory(pub Arc<Receiver>);

#[async_trait::async_trait]
impl ParticipantDirectory for ProcessDirectory {
    async fn resolve(&self, member: &wire::MemberKey) -> ExecutionResult<Participant> {
        let expected = &self.0.script.request.member;
        if member != expected {
            return Err(ExecutionError::EvidenceMismatch);
        }
        Ok(Participant::execution(
            expected.receiver().clone(),
            self.0.clone(),
        ))
    }
}
