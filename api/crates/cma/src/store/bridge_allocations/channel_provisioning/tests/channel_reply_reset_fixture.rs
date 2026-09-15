use super::*;

pub async fn reset(f: &ReplyFixture) -> AllocationRecord {
    let (admission, authority, management) = proposal(
        &f.svc,
        &f.bridges,
        &f.allocation.pin.bridge_id,
        "12345",
        Some(&f.allocation),
    )
    .await;
    f.allocations
        .reserve(
            AllocationReservation::reset(
                admission,
                authority,
                f.allocation.clone(),
                management,
                now(),
            )
            .unwrap(),
        )
        .await
        .unwrap()
}

pub struct Relay {
    pub original: AllocationRecord,
    pub command: AdmittedAgentCommand,
    pub message: String,
    pub calls: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl RelayDeliveryPort for Relay {
    async fn deliver(&self, request: RelayDeliveryRequest) -> DeliveryOutcome {
        request.require_current().unwrap();
        assert_eq!(request.item().source.allocation_id(), self.original.id);
        assert_eq!(
            request.item().source.target(),
            self.original.placement.target()
        );
        assert_eq!(
            request.item().source.command_id(),
            &self.command.receipt.envelope.command_id
        );
        assert_eq!(request.item().source.message_id(), self.message);
        assert_eq!(request.item().source.chat_id(), "-100123");
        assert_eq!(request.body()["message_id"], self.message);
        self.calls.lock().unwrap().push(request.item().id.clone());
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: format!("reset-original-{}", request.item().id),
            platform_message_id: None,
        })
    }
}
