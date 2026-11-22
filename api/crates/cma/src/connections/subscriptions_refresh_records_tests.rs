use super::*;
use crate::connections::{native_codex::DeviceLogin, subscription_store::PendingLogin};
use layer2_core::{trigger_domain::BrokerHandoffState, SubjectId};
use layer3_adapter::ingress_budget::{sizing, ClassBounds, IngressBudget};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("refresh candidate fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(4 * 1024 * 1024);
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap(),
    )
    .unwrap();
    drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
    (root, policy)
}

fn record(options: bool) -> SubscriptionRecord {
    SubscriptionRecord {
        owner: SubjectId::new("synthetic-owner"),
        incarnation: "00000000-0000-4000-8000-000000000001".into(),
        revision: 7,
        cache: options.then(|| SecretValue::new("synthetic-old-cache".repeat(1024))),
        pending: options.then(|| PendingLogin {
            intent: "synthetic-intent".into(),
            device: DeviceLogin {
                login_id: "synthetic-login".into(),
                verification_url: "https://example.invalid/device".into(),
                user_code: "synthetic-code".into(),
            },
            expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(600),
            heartbeat_at: SystemTime::UNIX_EPOCH,
            worker: "synthetic-worker".into(),
            state: BrokerHandoffState::Pending,
        }),
        refresh_lease: options.then(|| RefreshLease {
            holder: "previous-holder".repeat(1024),
            expires_at: SystemTime::UNIX_EPOCH,
        }),
    }
}

fn own(
    record: &SubscriptionRecord,
    policy: &IngressOwnershipPolicy,
) -> SharedOwned<SubscriptionRecord> {
    policy
        .admit(IngressClass::Data, record.copy_plan().unwrap())
        .unwrap()
        .initialize(|| record.clone())
}

fn holder(policy: &IngressOwnershipPolicy) -> SharedOwned<String> {
    let text = "synthetic-new-holder";
    policy
        .admit(
            IngressClass::Data,
            plan(sizing::shared_owner_bytes::<String>() + text.len()),
        )
        .unwrap()
        .initialize(|| text.to_owned())
}

fn secret(policy: &IngressOwnershipPolicy) -> SharedOwned<SecretValue> {
    policy
        .admit(
            IngressClass::Data,
            plan(sizing::shared_owner_bytes::<SecretValue>() + 8192),
        )
        .unwrap()
        .initialize(|| SecretValue::new("n".repeat(8192)))
}

fn assert_preserved(source: &SubscriptionRecord, next: &SubscriptionRecord) {
    assert_eq!(source.owner, next.owner);
    assert_eq!(source.incarnation, next.incarnation);
    assert_eq!(source.revision + 1, next.revision);
    assert_eq!(source.pending.is_some(), next.pending.is_some());
    if let (Some(source), Some(next)) = (&source.pending, &next.pending) {
        assert_eq!(source.intent, next.intent);
        assert_eq!(source.worker, next.worker);
        assert_eq!(source.device.login_id, next.device.login_id);
        assert_eq!(source.device.verification_url, next.device.verification_url);
        assert_eq!(source.device.user_code, next.device.user_code);
        assert_eq!(source.expires_at, next.expires_at);
        assert_eq!(source.heartbeat_at, next.heartbeat_at);
        assert_eq!(source.state, next.state);
    }
}

#[test]
fn claim_preserves_authority_cache_and_login_and_starts_the_requested_lease() {
    for options in [false, true] {
        let (root, policy) = policy();
        let baseline = root.available_bytes();
        let source = own(&record(options), &policy);
        let holder = holder(&policy);
        let start = SystemTime::now();
        let claimed =
            SubscriptionRecord::claim_admitted(&source, &holder, Duration::from_secs(30), &policy)
                .unwrap();
        let end = SystemTime::now();
        assert_preserved(&source, &claimed);
        assert_eq!(source.cache, claimed.cache);
        let lease = claimed.refresh_lease.as_ref().unwrap();
        assert_eq!(lease.holder, *holder);
        assert!(lease.expires_at >= start + Duration::from_secs(30));
        assert!(lease.expires_at <= end + Duration::from_secs(30));
        assert_ne!(lease.holder.as_ptr(), holder.as_ptr());
        assert!(claimed.belongs_to(&policy));
        assert_eq!(
            claimed.requested_bytes(),
            source.copy_plan().unwrap().requested_bytes() + holder.len()
        );
        assert_eq!(source.revision, 7);
        drop((source, holder, claimed));
        assert_eq!(root.available_bytes(), baseline);
    }
}

#[test]
fn completion_keeps_clears_or_replaces_cache_without_changing_other_authority() {
    for options in [false, true] {
        let (root, policy) = policy();
        let baseline = root.available_bytes();
        let source = own(&record(options), &policy);
        let cache = secret(&policy);
        for action in [
            CacheUpdate::Keep,
            CacheUpdate::Clear,
            CacheUpdate::Replace(&cache),
        ] {
            let (expected, extra) = match &action {
                CacheUpdate::Keep => (source.cache.as_ref(), 0),
                CacheUpdate::Clear => (None, 0),
                CacheUpdate::Replace(cache) => (Some(&***cache), cache.expose().len()),
            };
            let next = SubscriptionRecord::finish_admitted(&source, action, &policy).unwrap();
            assert_preserved(&source, &next);
            assert!(next.refresh_lease.is_none());
            assert_eq!(next.cache.as_ref(), expected);
            assert_eq!(
                next.requested_bytes(),
                source.copy_plan().unwrap().requested_bytes() + extra
            );
            assert!(next.belongs_to(&policy));
        }
        assert_eq!(source.revision, 7);
        drop((source, cache));
        assert_eq!(root.available_bytes(), baseline);
    }
}

#[test]
fn materialized_data_pressure_refuses_all_candidate_construction() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let source = own(&record(true), &policy);
    let holder = holder(&policy);
    let cache = secret(&policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![3u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    for _ in 0..32 {
        assert!(matches!(
            SubscriptionRecord::claim_admitted(&source, &holder, Duration::from_secs(30), &policy),
            Err(AdmissionRefusal::AtCapacity { .. })
        ));
        for action in [
            CacheUpdate::Keep,
            CacheUpdate::Clear,
            CacheUpdate::Replace(&cache),
        ] {
            assert!(matches!(
                SubscriptionRecord::finish_admitted(&source, action, &policy),
                Err(AdmissionRefusal::AtCapacity { .. })
            ));
        }
    }
    assert_eq!(pressure[pressure.len() - 1], 3);
    assert_eq!(source.revision, 7);
    drop((pressure, source, holder, cache));
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn record_holder_and_replacement_cache_require_the_same_root_and_attachment() {
    let (_, foreign) = policy();
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 2, plan(1), plan(1))
            .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let first = parent.admit_attachment().unwrap();
    let second = parent.admit_attachment().unwrap();
    let receiving = first.policy();
    let source = own(&record(true), receiving);
    let local_holder = holder(receiving);
    let local_cache = secret(receiving);
    for wrong in [&foreign, second.policy(), &parent] {
        let foreign_source = own(&record(true), wrong);
        let foreign_holder = holder(wrong);
        let foreign_cache = secret(wrong);
        let before = root.available_bytes();
        for (record, holder) in [(&foreign_source, &local_holder), (&source, &foreign_holder)] {
            assert!(matches!(
                SubscriptionRecord::claim_admitted(
                    record,
                    holder,
                    Duration::from_secs(30),
                    receiving
                ),
                Err(AdmissionRefusal::InvalidPlan { .. })
            ));
        }
        for action in [
            CacheUpdate::Keep,
            CacheUpdate::Clear,
            CacheUpdate::Replace(&local_cache),
        ] {
            assert!(matches!(
                SubscriptionRecord::finish_admitted(&foreign_source, action, receiving),
                Err(AdmissionRefusal::InvalidPlan { .. })
            ));
        }
        assert!(matches!(
            SubscriptionRecord::finish_admitted(
                &source,
                CacheUpdate::Replace(&foreign_cache),
                receiving
            ),
            Err(AdmissionRefusal::InvalidPlan { .. })
        ));
        assert_eq!(root.available_bytes(), before);
    }
}

#[test]
fn candidates_refuse_attachment_downgrade_in_each_input() {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 1, plan(1), plan(1))
            .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let attachment = parent.admit_attachment().unwrap();
    let process_source = own(&record(true), &parent);
    let process_holder = holder(&parent);
    let process_cache = secret(&parent);
    let attached_source = own(&record(true), attachment.policy());
    let attached_holder = holder(attachment.policy());
    let attached_cache = secret(attachment.policy());
    let before = root.available_bytes();
    for (source, holder) in [
        (&attached_source, &process_holder),
        (&process_source, &attached_holder),
        (&attached_source, &attached_holder),
    ] {
        assert!(matches!(
            SubscriptionRecord::claim_admitted(source, holder, Duration::from_secs(30), &parent),
            Err(AdmissionRefusal::InvalidPlan { .. })
        ));
    }
    for action in [
        CacheUpdate::Keep,
        CacheUpdate::Clear,
        CacheUpdate::Replace(&process_cache),
    ] {
        assert!(matches!(
            SubscriptionRecord::finish_admitted(&attached_source, action, &parent),
            Err(AdmissionRefusal::InvalidPlan { .. })
        ));
    }
    assert!(matches!(
        SubscriptionRecord::finish_admitted(
            &process_source,
            CacheUpdate::Replace(&attached_cache),
            &parent
        ),
        Err(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(root.available_bytes(), before);
}

#[test]
fn candidates_keep_their_attachment_after_sources_and_intermediate_clones_drop() {
    for claiming in [false, true] {
        let root = IngressBudget::new(128 * 1024 * 1024);
        let bounds =
            IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 1, plan(1), plan(1))
                .unwrap();
        let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
        let baseline = root.available_bytes();
        let attachment = parent.admit_attachment().unwrap();
        let source = own(&record(true), attachment.policy());
        let holder = holder(attachment.policy());
        let cache = secret(attachment.policy());
        let candidate = if claiming {
            SubscriptionRecord::claim_admitted(
                &source,
                &holder,
                Duration::from_secs(30),
                attachment.policy(),
            )
            .unwrap()
        } else {
            SubscriptionRecord::finish_admitted(
                &source,
                CacheUpdate::Replace(&cache),
                attachment.policy(),
            )
            .unwrap()
        };
        let last = candidate.clone();
        let charge = last.requested_bytes();
        drop((candidate, source, holder, cache, attachment));
        assert!(baseline - root.available_bytes() >= charge);
        assert!(parent.admit_attachment().is_err());
        assert_eq!(last.revision, 8);
        drop(last);
        drop(parent.admit_attachment().unwrap());
        assert_eq!(root.available_bytes(), baseline);
    }
}

#[test]
fn panic_during_candidate_initialization_returns_all_new_capacity() {
    let (root, policy) = policy();
    let source = own(&record(true), &policy);
    let holder = holder(&policy);
    let baseline = root.available_bytes();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SubscriptionRecord::claim_admitted(&source, &holder, Duration::MAX, &policy)
    }));
    assert!(
        result.is_err(),
        "unrepresentable lease timestamp must unwind"
    );
    assert_eq!(root.available_bytes(), baseline);
    assert_eq!(source.revision, 7);
}
