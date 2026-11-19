use super::fixture::{binding, policy, receipt};
use super::*;
use futures_util::FutureExt;
use std::sync::atomic::Ordering::SeqCst;

#[test]
fn admitted_guidance_preserves_owner_bytes_and_exact_replay() {
    let (root, policy) = policy();
    let (binding, source) = binding(0, "Old managed\n\nOwner exact text  \n", None);
    let receipt = receipt(&binding, &policy);
    let previous = vec!["Old".into(), "Old managed".into()];
    for _ in 0..2 {
        binding
            .append_guidance_admitted(
                "conversation",
                1,
                &receipt,
                "New managed",
                &previous,
                &policy,
            )
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(
            *source.text.lock().unwrap(),
            "New managed\n\nOwner exact text  \n"
        );
        assert_eq!(source.writes.load(SeqCst), 1);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[test]
fn admitted_guidance_preserves_cas_lost_ack_and_validation_failures() {
    for fault in 1..=16 {
        if fault == 10 {
            continue;
        }
        let (root, policy) = policy();
        let (binding, source) = binding(fault, "Old managed\n\nOwner exact text  \n", None);
        let receipt = if fault >= 15 {
            "stale".into()
        } else {
            receipt(&binding, &policy)
        };
        let error = binding
            .append_guidance_admitted(
                "conversation",
                1,
                &receipt,
                "New managed",
                &["Old managed".into()],
                &policy,
            )
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert!(error.belongs_to(&policy));
        assert_eq!(
            source.writes.load(SeqCst),
            usize::from(matches!(fault, 1..=3 | 13 | 14))
        );
        match fault {
            1 => assert!(
                matches!(error.as_adapter(), Some(AdapterError::Rejected(detail)) if detail == "file changed concurrently")
            ),
            2 => assert!(
                matches!(error.as_adapter(), Some(AdapterError::Unavailable(detail)) if detail == "write ACK lost")
            ),
            11 => assert!(matches!(
                error.as_adapter(),
                Some(AdapterError::RuntimeLedgerHold(_))
            )),
            16 => assert!(matches!(
                error.as_adapter(),
                Some(AdapterError::Unsupported(_))
            )),
            _ => assert!(
                matches!(error.as_adapter(), Some(AdapterError::Rejected(_))),
                "fault={fault}: {error:?}"
            ),
        }
        if fault == 1 {
            assert_eq!(*source.text.lock().unwrap(), "Concurrent owner edit");
        }
        if matches!(fault, 2 | 3 | 13 | 14) {
            binding
                .append_guidance_admitted(
                    "conversation",
                    1,
                    &receipt,
                    "New managed",
                    &["Old managed".into()],
                    &policy,
                )
                .unwrap()
                .now_or_never()
                .unwrap()
                .unwrap();
            assert_eq!(
                source.writes.load(SeqCst),
                1,
                "uncertain write must not duplicate"
            );
        }
        let clone = error.clone();
        drop((binding, source, error));
        assert_eq!(
            root.capacity_bytes() - root.available_bytes(),
            clone.requested_bytes()
        );
        drop(clone);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[test]
fn admitted_guidance_refuses_invalid_receipts_history_and_unsupported_sources() {
    let (root, policy) = policy();
    let (binding, source) = binding(0, "owner", None);
    let receipt = receipt(&binding, &policy);
    for (sequence, proof, guidance, previous) in [
        (0, receipt.as_str(), "new", vec![]),
        (1, "wrong", "new", vec![]),
        (1, receipt.as_str(), "", vec![]),
        (1, receipt.as_str(), "new", vec!["".into()]),
        (1, receipt.as_str(), "new", vec!["old".into(); 9]),
    ] {
        let error = binding
            .append_guidance_admitted(
                "conversation",
                sequence,
                proof,
                guidance,
                &previous,
                &policy,
            )
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error.as_adapter(),
            Some(AdapterError::Unsupported(_) | AdapterError::OperationIdentityConflict(_))
        ));
        assert_eq!(source.writes.load(SeqCst), 0);
        drop(error);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
    let (legacy, files) = super::super::writer_tests::binding(0);
    let error = legacy
        .append_guidance_admitted("conversation", 1, &receipt, "new", &[], &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, AdmittedError::Unsupported(_)));
    assert_eq!(files.status_reads.load(SeqCst), 0);
    let (_, foreign) = self::policy();
    let (binding, source) = self::binding(0, "owner", Some(foreign));
    let error = binding
        .append_guidance_admitted("conversation", 1, &receipt, "new", &[], &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(error.refusal().is_some());
    assert_eq!(source.reads.load(SeqCst), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn pending_guidance_write_retains_data_and_releases_transformation_on_cancellation() {
    let (root, policy) = policy();
    let (binding, source) = binding(10, "owner", None);
    let receipt = receipt(&binding, &policy);
    let mut operation = Box::pin(
        binding
            .append_guidance_admitted("conversation", 1, &receipt, "new", &[], &policy)
            .unwrap(),
    );
    assert!(operation.as_mut().now_or_never().is_none());
    assert_eq!(source.writes.load(SeqCst), 1);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(operation);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
