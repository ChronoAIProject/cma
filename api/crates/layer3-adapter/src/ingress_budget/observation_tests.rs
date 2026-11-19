use super::*;
use crate::ingress_budget::{
    AdmittedWatchSender, AllocationPlan, ClassBounds, IngressBudget, IngressClass,
    IngressOwnershipPolicy,
};
use crate::{
    ConversationInput, ConversationInputSender, ConversationSendOutcome,
    ConversationSettingsSupport,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, Semaphore};

struct Support(DeliveryObservation);

#[async_trait::async_trait]
impl ConversationSettingsSupport for Support {
    async fn delivery_observation(&self) -> DeliveryObservation {
        self.0.clone()
    }

    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn managed_context_progress(&self) -> Option<watch::Receiver<u64>> {
        panic!("admitted delivery must use the owned observation port")
    }

    async fn retained_history_deadline(&self) -> Option<watch::Receiver<Option<Instant>>> {
        panic!("admitted delivery must use the owned observation port")
    }
}

#[tokio::test(start_paused = true)]
async fn owned_observers_preserve_delivery_progress_and_cancellation() {
    for cancel in [false, true] {
        for retained_history in [false, true] {
            delivery_case(cancel, retained_history).await;
        }
    }
}

async fn delivery_case(cancel: bool, retained_history: bool) {
    let root = IngressBudget::new(128 * 1024);
    let floor = AllocationPlan::checked("delivery fixture", &[4096]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    let (progress, received) =
        AdmittedWatchSender::channel(&policy, IngressClass::Data, 0).unwrap();
    let (history, loading) =
        AdmittedWatchSender::channel(&policy, IngressClass::Data, None).unwrap();
    let support = Arc::new(Support(DeliveryObservation::admitted(
        Some(received),
        Some(loading),
    )));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let sender = ConversationInputSender::spawn(1, {
        let entered = entered.clone();
        let release = release.clone();
        move |_| {
            let entered = entered.clone();
            let release = release.clone();
            async move {
                entered.notify_one();
                release.acquire().await.unwrap().forget();
                Ok(())
            }
        }
    })
    .with_settings_support(support.clone());
    let operation = tokio::spawn({
        let sender = sender.clone();
        async move { sender.send_classified(input(retained_history)).await }
    });
    entered.notified().await;
    if retained_history {
        history.send_replace(Some(Instant::now() + Duration::from_secs(120)));
    }
    tokio::task::yield_now().await;
    for cursor in 1..=4 {
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert!(!operation.is_finished());
        progress.send_replace(cursor);
        tokio::task::yield_now().await;
    }
    drop((support, progress, history, sender));
    assert!(root.available_bytes() < root.capacity_bytes());
    if cancel {
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
    } else {
        release.add_permits(1);
        assert!(matches!(
            operation.await.unwrap(),
            ConversationSendOutcome::WriteConfirmed
        ));
    }
    tokio::task::yield_now().await;
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

fn input(retained_history: bool) -> ConversationInput {
    if retained_history {
        ConversationInput::Configure {
            model: "synthetic".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Guarded,
            system_prompt: None,
        }
    } else {
        ConversationInput::RefreshContext {
            service_origin: None,
            operation_sequence: 1,
            readiness_receipt: "synthetic".into(),
            previous_guidance: Vec::new(),
            working_directory: None,
            guidance: "synthetic".into(),
            model: "synthetic".into(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Guarded,
        }
    }
}
