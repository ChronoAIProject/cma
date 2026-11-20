use super::*;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(256 * 1024);
    let floor = AllocationPlan::checked("input fixture", &[32 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn input(policy: &IngressOwnershipPolicy, model: &str) -> OwnedWork<ConversationInput> {
    let plan = AllocationPlan::checked(
        "input payload",
        &[model.len(), sizing::shared_owner_bytes::<AdapterError>()],
    )
    .unwrap();
    IngressWorkScope::new(policy.admit(IngressClass::Data, plan).unwrap()).initialize_owned(|| {
        ConversationInput::Configure {
            model: model.to_owned(),
            reasoning_effort: crate::ReasoningEffort::High,
            permission: crate::PermissionLevel::Guarded,
            system_prompt: None,
        }
    })
}

struct Guard {
    entered: AtomicBool,
    suspend: bool,
    allowed: AtomicBool,
}

impl Guard {
    fn new(allowed: bool, suspend: bool) -> Arc<Self> {
        Arc::new(Self {
            entered: AtomicBool::new(false),
            suspend,
            allowed: AtomicBool::new(allowed),
        })
    }
    async fn check_inner(&self) -> crate::Result<()> {
        self.entered.store(true, Ordering::Release);
        if self.suspend {
            std::future::pending::<()>().await;
        }
        if self.allowed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(AdapterError::Rejected("synthetic authority expired".into()))
        }
    }
}

#[async_trait::async_trait]
impl ConversationInputGuard for Guard {
    fn admitted_plan(&self) -> Result<AllocationPlan, AdmittedError> {
        let future = self.check_inner();
        // async-trait boxes the wrapper awaiting check_inner: its inline future,
        // self reference, discriminant and padding; the diagnostic shares once.
        Ok(AllocationPlan::checked(
            "authority fixture",
            &[
                std::mem::size_of_val(&future)
                    + std::mem::align_of_val(&future)
                    + 2 * size_of::<&Self>(),
                sizing::shared_owner_bytes::<AdapterError>(),
                "synthetic authority expired".len(),
            ],
        )?)
    }
    async fn check(&self) -> crate::Result<()> {
        self.check_inner().await
    }
}

async fn entered(guard: &Guard) {
    while !guard.entered.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn runtime_errors_retain_input_storage_until_the_last_result_clone() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(2, policy.clone()).unwrap();
    let driver = tokio::spawn(receiver.run(|input| async move {
        let result = input
            .map(|input| async move {
                match input {
                    ConversationInput::Configure { model, .. } => {
                        Err(AdapterError::Unsupported(model))
                    }
                    _ => Ok(()),
                }
            })
            .resolve()
            .await
            .transpose();
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error.into_shared_admitted()?.into()),
        }
    }));
    let baseline = root.available_bytes();
    let outcome = sender
        .send_classified(input(&policy, &"x".repeat(4096)))
        .await;
    assert!(
        matches!(&outcome, AdmittedConversationSendOutcome::PreWriteFailure(error) if matches!(error.as_adapter(), Some(AdapterError::Unsupported(message)) if message.len() == 4096))
    );
    let held = root.available_bytes();
    assert!(held < baseline);
    let clone = outcome.clone();
    drop(outcome);
    assert_eq!(root.available_bytes(), held);
    drop(clone);
    assert_eq!(root.available_bytes(), baseline);
    assert!(!sender.is_closed());
    drop(sender);
    driver.await.unwrap();
    root.assert_idle();
}

#[tokio::test]
async fn authority_refusal_is_owned_and_leaves_the_ordered_writer_usable() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(2, policy.clone()).unwrap();
    let writes = Arc::new(AtomicUsize::new(0));
    let recorded = writes.clone();
    let driver = tokio::spawn(receiver.run(move |_input| {
        recorded.fetch_add(1, Ordering::AcqRel);
        async { Ok(()) }
    }));
    let outcome = sender
        .send_guarded(input(&policy, "first"), Guard::new(false, false))
        .await;
    assert!(
        matches!(&outcome, AdmittedConversationSendOutcome::PreWriteFailure(error) if matches!(error.as_adapter(), Some(AdapterError::Rejected(_))))
    );
    assert_eq!(writes.load(Ordering::Acquire), 0);
    assert!(matches!(
        sender
            .send_guarded(input(&policy, "second"), Guard::new(true, false))
            .await,
        AdmittedConversationSendOutcome::WriteConfirmed
    ));
    assert_eq!(writes.load(Ordering::Acquire), 1);
    drop(sender);
    driver.await.unwrap();
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(outcome);
    root.assert_idle();
}

#[tokio::test]
async fn cancelling_an_authority_check_releases_the_input_and_keeps_the_writer() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let driver = tokio::spawn(receiver.run(|_input| async { Ok(()) }));
    let baseline = root.available_bytes();
    let guard = Guard::new(true, true);
    let operation = tokio::spawn({
        let sender = sender.clone();
        let input = input(&policy, &"x".repeat(4096));
        let guard = guard.clone();
        async move { sender.send_guarded(input, guard).await }
    });
    entered(&guard).await;
    assert!(root.available_bytes() < baseline);
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    assert!(matches!(
        sender.send_classified(input(&policy, "next")).await,
        AdmittedConversationSendOutcome::WriteConfirmed
    ));
    assert_eq!(root.available_bytes(), baseline);
    drop(sender);
    driver.await.unwrap();
    root.assert_idle();
}

#[tokio::test]
async fn cancelling_after_handler_entry_retires_the_writer_and_drops_owned_input() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let notified = started.clone();
    let driver = tokio::spawn(receiver.run(move |input| {
        let notified = notified.clone();
        async move {
            let _result = input
                .map(|input| async move {
                    notified.notify_one();
                    std::future::pending::<()>().await;
                    input
                })
                .resolve()
                .await;
            Ok(())
        }
    }));
    let operation = tokio::spawn({
        let sender = sender.clone();
        let input = input(&policy, &"x".repeat(4096));
        async move { sender.send_classified(input).await }
    });
    started.notified().await;
    operation.abort();
    assert!(operation.await.unwrap_err().is_cancelled());
    driver.await.unwrap();
    assert!(sender.is_closed());
    drop(sender);
    root.assert_idle();
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_queued_input_keeps_storage_until_the_receiver_drops_it() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let baseline = root.available_bytes();
    let outcome = sender
        .send_classified(input(&policy, &"x".repeat(4096)))
        .await;
    assert!(matches!(
        outcome,
        AdmittedConversationSendOutcome::PreWriteFailure(AdmittedError::Unavailable(_))
    ));
    assert!(
        root.available_bytes() < baseline,
        "cancelled queue still owns the input"
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let driver = tokio::spawn(receiver.run(move |_| {
        observed.fetch_add(1, Ordering::AcqRel);
        async { Ok(()) }
    }));
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(root.available_bytes(), baseline);
    assert!(matches!(
        sender.send_classified(input(&policy, "next")).await,
        AdmittedConversationSendOutcome::WriteConfirmed
    ));
    assert_eq!(calls.load(Ordering::Acquire), 1);
    drop(sender);
    driver.await.unwrap();
    root.assert_idle();
}

#[tokio::test]
async fn unsupported_guard_is_refused_without_constructing_its_check_future() {
    struct Legacy(AtomicBool);
    #[async_trait::async_trait]
    impl ConversationInputGuard for Legacy {
        async fn check(&self) -> crate::Result<()> {
            self.0.store(true, Ordering::Release);
            Err(AdapterError::Rejected("legacy check must not run".into()))
        }
    }
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let driver = tokio::spawn(receiver.run(|_| async { Ok(()) }));
    let guard = Arc::new(Legacy(AtomicBool::new(false)));
    let result = sender
        .send_guarded(input(&policy, "first"), guard.clone())
        .await;
    assert!(matches!(
        result,
        AdmittedConversationSendOutcome::PreWriteFailure(AdmittedError::Unsupported(_))
    ));
    assert!(!guard.0.load(Ordering::Acquire));
    assert!(matches!(
        sender.send_classified(input(&policy, "next")).await,
        AdmittedConversationSendOutcome::WriteConfirmed
    ));
    drop(sender);
    driver.await.unwrap();
    root.assert_idle();
}

#[tokio::test]
async fn queued_authority_is_rechecked_after_the_previous_handler_finishes() {
    let (_, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(2, policy.clone()).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = tokio::spawn(receiver.run({
        let entered = entered.clone();
        let release = release.clone();
        let calls = calls.clone();
        move |input| {
            let entered = entered.clone();
            let release = release.clone();
            let calls = calls.clone();
            async move {
                let _held = input;
                calls.fetch_add(1, Ordering::AcqRel);
                entered.notify_one();
                release.acquire().await.unwrap().forget();
                Ok(())
            }
        }
    }));
    let first = tokio::spawn({
        let sender = sender.clone();
        let input = input(&policy, "first");
        async move { sender.send_classified(input).await }
    });
    entered.notified().await;
    let guard = Guard::new(true, false);
    {
        let second = sender.send_guarded(input(&policy, "second"), guard.clone());
        tokio::pin!(second);
        tokio::select! { biased; _ = &mut second => panic!("queued request cannot finish"), _ = tokio::task::yield_now() => () }
        assert!(!guard.entered.load(Ordering::Acquire));
        guard.allowed.store(false, Ordering::Release);
        release.add_permits(1);
        assert!(matches!(
            first.await.unwrap(),
            AdmittedConversationSendOutcome::WriteConfirmed
        ));
        assert!(matches!(
            second.await,
            AdmittedConversationSendOutcome::PreWriteFailure(_)
        ));
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }
    drop(sender);
    driver.await.unwrap();
}

#[path = "conversation_admitted_allocation_tests.rs"]
mod allocation;

#[path = "conversation_admitted_delivery_tests.rs"]
mod delivery;

#[test]
fn identity_clones_retain_storage_and_cannot_mix_process_roots() {
    let (root, policy) = fixture();
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let plan = AllocationPlan::checked(
        "conversation identity",
        &["conversation".len(), sizing::shared_owner_bytes::<String>()],
    )
    .unwrap();
    let identity = policy
        .admit(IngressClass::Data, plan)
        .unwrap()
        .initialize(|| "conversation".to_owned());
    let channel = AdmittedConversationChannel::new(sender, identity).unwrap();
    assert_eq!(channel.conversation_id(), "conversation");
    let clone = channel.identity().clone();
    drop((channel, receiver));
    assert_eq!(root.reserved_work_bytes(), plan.requested_bytes());
    drop(clone);
    root.assert_idle();

    let (other_root, other_policy) = fixture();
    let identity = other_policy
        .admit(IngressClass::Data, plan)
        .unwrap()
        .initialize(|| "conversation".to_owned());
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy).unwrap();
    assert!(matches!(
        AdmittedConversationChannel::new(sender, identity),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    other_root.assert_idle();
    drop(receiver);
    root.assert_idle();
}

#[tokio::test]
async fn a_foreign_outcome_retires_the_writer_without_a_false_prewrite_claim() {
    let (root, policy) = fixture();
    let (other_root, other_policy) = fixture();
    let plan = AllocationPlan::checked(
        "foreign error",
        &[
            "unsupported".len(),
            sizing::shared_owner_bytes::<AdapterError>(),
        ],
    )
    .unwrap();
    let error = other_policy
        .admit(IngressClass::Data, plan)
        .unwrap()
        .initialize(|| AdapterError::Unsupported("unsupported".into()));
    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let driver = tokio::spawn(receiver.run(move |_| {
        let error = error.clone();
        async move { Err(error.into()) }
    }));
    assert!(matches!(
        sender.send_classified(input(&policy, "request")).await,
        AdmittedConversationSendOutcome::Ambiguous(AdmittedError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
    driver.await.unwrap();
    assert!(sender.is_closed());
    drop(sender);
    root.assert_idle();
    other_root.assert_idle();
}
