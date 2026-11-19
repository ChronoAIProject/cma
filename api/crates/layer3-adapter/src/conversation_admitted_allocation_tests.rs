use super::*;
use crate::allocation_probe::AllocationProbe;
use futures_util::FutureExt;

#[test]
fn isolated_queue_storage_and_refusal_contract() {
    const TEST: &str = "conversation_channel::admitted::tests::allocation::isolated_queue_storage_and_refusal_contract";
    if std::env::var_os("CMA_INPUT_QUEUE_ALLOCATION_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env("CMA_INPUT_QUEUE_ALLOCATION_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    let (root, policy) = fixture();
    drop(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("warm pools", &[1]).unwrap(),
            )
            .unwrap(),
    );
    let mut probe = AllocationProbe::new();
    let (sender, receiver) =
        probe.track(|| AdmittedConversationInputSender::channel(3, policy.clone()).unwrap());
    let held = root.capacity_bytes() - root.available_bytes();
    assert!(probe.peak() > 0);
    assert!(probe.peak() <= held);
    let clone = probe.track(|| sender.clone());
    let peak = probe.peak();
    probe.track(|| drop((sender, clone)));
    assert!(probe.live() > 0, "the receiver still owns queue storage");
    assert_eq!(root.capacity_bytes() - root.available_bytes(), held);
    probe.track(|| drop(receiver));
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert!(probe.peak() <= held);
    eprintln!("input_queue requested_peak={peak} admitted={held}");
    drop(probe);

    let allowance = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "legacy queue allowance",
                &[sizing::channel_bytes_checked::<u8>(3, 3).unwrap()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let (legacy_sender, legacy_receiver) = probe.track(|| mpsc::channel::<u8>(3));
    drop((legacy_sender, allowance));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert!(
        probe.live() > 0,
        "a sender-only allowance releases while raw receiver storage is live"
    );
    eprintln!(
        "raw_input_queue_after_sender_drop retained={}",
        probe.live()
    );
    drop(legacy_receiver);
    assert_eq!(probe.live(), 0);
    drop(probe);

    let guard = Guard::new(false, false);
    let plan = guard.admitted_plan().unwrap();
    let scope = IngressWorkScope::new(policy.admit(IngressClass::Data, plan).unwrap());
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| scope.run(guard.check()).now_or_never().unwrap().transpose());
    let error = probe.track(|| result.unwrap_err().into_shared_admitted().unwrap());
    assert!(probe.peak() <= plan.requested_bytes());
    assert!(probe.live() >= "synthetic authority expired".len());
    eprintln!(
        "authority_check requested_peak={} retained={} admitted={}",
        probe.peak(),
        probe.live(),
        plan.requested_bytes()
    );
    drop(error);
    assert_eq!(probe.live(), 0);
    drop((probe, scope));
    assert_eq!(root.available_bytes(), root.capacity_bytes());

    let (sender, receiver) = AdmittedConversationInputSender::channel(1, policy.clone()).unwrap();
    let inputs: [_; 128] = std::array::from_fn(|_| input(&policy, "retained input"));
    let scopes: [_; 128] = std::array::from_fn(|i| inputs[i].work_scope().unwrap());
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("input pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let mut probe = AllocationProbe::new();
    let results =
        probe.track(|| inputs.map(|input| sender.send_classified(input).now_or_never().unwrap()));
    let copies = probe.track(|| results.clone());
    assert!(results.iter().all(|result| matches!(
        result,
        AdmittedConversationSendOutcome::PreWriteFailure(AdmittedError::Refused(
            AdmissionRefusal::AtCapacity { .. }
        ))
    )));
    assert_eq!(
        probe.allocated(),
        0,
        "pressure refusal must precede metadata allocation"
    );
    assert_eq!(probe.live(), 0);
    assert!(matches!(
        probe.track(|| AdmittedConversationInputSender::channel(usize::MAX, policy.clone())),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(
        probe.allocated(),
        0,
        "overflow refusal must be allocation-free"
    );
    eprintln!(
        "input_refusals requested_peak={} retained={} churn={}",
        probe.peak(),
        probe.live(),
        probe.allocated()
    );
    drop((results, copies, probe, pressure, scopes, sender, receiver));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
