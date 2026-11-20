use super::*;

#[test]
fn admitted_context_keeps_transport_authority_and_refuses_legacy_installation() {
    for provider in [false, true] {
        let (root, policy) = super::super::super::guidance_admission::fixture::policy();
        let (binding, source) = binding(&policy, provider, 6, None);
        let raw = if provider {
            ToolTransport::Provider(Box::new(source.provider.clone()))
        } else {
            ToolTransport::Access(source.access.clone())
        };
        assert!(matches!(
            binding
                .confirm_transport(&raw, "conversation", 1)
                .now_or_never()
                .unwrap(),
            Err(AdapterError::Unsupported(_))
        ));
        assert_eq!(source.confirmations.load(SeqCst), 0);
        binding
            .confirm_transport_admitted(&raw, "conversation", 1, &policy)
            .now_or_never()
            .unwrap()
            .unwrap();
        let prepared = binding
            .prepare_transport_admitted(
                "conversation",
                1,
                Some("https://new.example.test"),
                &policy,
            )
            .now_or_never()
            .unwrap()
            .unwrap()
            .unwrap();
        binding
            .confirm_transport_admitted(&prepared, "conversation", 1, &policy)
            .now_or_never()
            .unwrap()
            .unwrap();
        drop(prepared);
        assert!(root.available_bytes() < root.capacity_bytes());
        let legacy = binding
            .prepare_transport("conversation", 1, Some("https://new.example.test"))
            .now_or_never()
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            binding
                .confirm_transport(&legacy, "conversation", 1)
                .now_or_never()
                .unwrap(),
            Err(AdapterError::Unsupported(_))
        ));
        assert_eq!(source.preparations.load(SeqCst), 1);
        assert_eq!(source.confirmations.load(SeqCst), 2);
        drop((binding, source, legacy));
        root.assert_idle();
    }
}

#[test]
fn binding_snapshot_admits_nested_strings_and_refuses_before_copying() {
    if deallocation::isolated("hosted::managed_context::transport_admission::tests::snapshot_tests::binding_snapshot_admits_nested_strings_and_refuses_before_copying") { return; }
    let (root, policy) = super::super::super::guidance_admission::fixture::policy();
    let request = crate::AgentOpenRequest::new(
        "o".repeat(1024),
        crate::RuntimeOptions {
            model: Some("m".repeat(1024)),
            working_directory: "w".repeat(4096),
            home_directory: "h".repeat(4096),
            slot: crate::AgentSlot::new("a".repeat(1024)),
            ..Default::default()
        },
        Some("conversation".into()),
    );
    let runtime = RuntimeHandle("r".repeat(4096));
    let proxy = crate::ToolProxyRequest {
        sandbox_id: "s".repeat(256),
        generation: 1,
        endpoint: "https://gateway.example.test/mcp".into(),
        credential_generation: 1,
    };
    let boot = "b".repeat(32);
    let mut provider = HostedProviderToolBinding::new(
        "a".repeat(64),
        runtime.clone(),
        request.clone(),
        1,
        proxy.clone(),
        Some(crate::hosted::HostedToolOpenReceipt {
            boot_id: boot.clone(),
            conversation: "conversation".into(),
        }),
    )
    .unwrap()
    .with_transport(
        "https://relay.example.test",
        crate::RuntimeCredential::new(format!("cma_pt_{}_{}", "a".repeat(64), "b".repeat(64))),
    )
    .unwrap();
    provider
        .observe(crate::ToolProxyStatus {
            request: proxy,
            boot_id: boot,
        })
        .unwrap();
    let access = HostedAccessToolBinding::new(
        "https://access.example.test",
        crate::RuntimeCredential::new(format!("cma_at_{}", "a".repeat(64))),
        runtime,
        &request,
        1,
        None,
    )
    .unwrap()
    .with_prepared_conversation("conversation")
    .unwrap();
    for raw in [
        ToolTransport::Access(access),
        ToolTransport::Provider(Box::new(provider)),
    ] {
        let mut probe = AllocationProbe::new();
        let snapshot = probe.track(|| raw.clone_admitted(&policy).unwrap());
        let charged = root.reserved_work_bytes();
        assert!(probe.peak() > 4096);
        assert!(
            probe.peak() <= charged,
            "peak={} charged={charged}",
            probe.peak()
        );
        match (&raw, &snapshot) {
            (ToolTransport::Access(a), ToolTransport::AdmittedAccess(b)) => assert_eq!(a, &**b),
            (ToolTransport::Provider(a), ToolTransport::AdmittedProvider(b)) => {
                assert_eq!(&**a, &**b)
            }
            _ => unreachable!(),
        }
        let clone = snapshot.clone_admitted(&policy).unwrap();
        drop(snapshot);
        assert_eq!(root.reserved_work_bytes(), charged);
        drop(clone);
        assert_eq!(probe.live(), 0);
        drop(probe);
        let pressure = policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "snapshot pressure",
                    &[policy.budget(IngressClass::Data).available_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| raw.clone_admitted(&policy).err().unwrap());
        assert!(error.refusal().is_some());
        assert_eq!(probe.allocated(), 0);
        drop(pressure);
        root.assert_idle();
    }
}
