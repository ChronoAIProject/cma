use super::*;
use crate::connections::{
    native_codex::DeviceLogin,
    subscription_store::{PendingLogin, RefreshLease, SubscriptionStore},
    subscriptions::{NativeLogin, NativeSubscriptionAuth},
    SubscriptionError,
};
use layer2_core::{trigger_domain::BrokerHandoffState, SubjectId};
use layer3_adapter::ingress_budget::{AdmittedError, ClassBounds, IngressBudget};
use std::time::{Duration, SystemTime};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("subscription fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(4 * 1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    drop(policy.admit(IngressClass::Data, plan(1)).unwrap());
    (root, policy)
}

fn record(cache: bool, pending: bool, lease: bool) -> SubscriptionRecord {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
    SubscriptionRecord {
        owner: SubjectId::new("synthetic-owner"),
        incarnation: "00000000-0000-4000-8000-000000000001".into(),
        revision: 7,
        cache: cache.then(|| SecretValue::new("synthetic-cache".repeat(1024))),
        pending: pending.then(|| PendingLogin {
            intent: "synthetic-intent".into(),
            device: DeviceLogin {
                login_id: "synthetic-login".into(),
                verification_url: "https://example.invalid/device".into(),
                user_code: "synthetic-code".into(),
            },
            expires_at: now + Duration::from_secs(600),
            heartbeat_at: now,
            worker: "synthetic-worker".into(),
            state: BrokerHandoffState::Pending,
        }),
        refresh_lease: lease.then(|| RefreshLease {
            holder: "synthetic-holder".into(),
            expires_at: now + Duration::from_secs(30),
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

fn assert_same(left: &SubscriptionRecord, right: &SubscriptionRecord) {
    assert_eq!(left.owner, right.owner);
    assert_eq!(left.incarnation, right.incarnation);
    assert_eq!(left.revision, right.revision);
    assert_eq!(left.cache, right.cache);
    assert_eq!(left.pending.is_some(), right.pending.is_some());
    if let (Some(left), Some(right)) = (&left.pending, &right.pending) {
        assert_eq!(left.intent, right.intent);
        assert_eq!(left.device.login_id, right.device.login_id);
        assert_eq!(left.device.verification_url, right.device.verification_url);
        assert_eq!(left.device.user_code, right.device.user_code);
        assert_eq!(left.expires_at, right.expires_at);
        assert_eq!(left.heartbeat_at, right.heartbeat_at);
        assert_eq!(left.worker, right.worker);
        assert_eq!(left.state, right.state);
    }
    assert_eq!(left.refresh_lease.is_some(), right.refresh_lease.is_some());
    if let (Some(left), Some(right)) = (&left.refresh_lease, &right.refresh_lease) {
        assert_eq!(left.holder, right.holder);
        assert_eq!(left.expires_at, right.expires_at);
    }
}

#[test]
fn admitted_record_copy_preserves_every_optional_field_and_authority() {
    for cache in [false, true] {
        for pending in [false, true] {
            for lease in [false, true] {
                let (root, policy) = policy();
                let baseline = root.available_bytes();
                let raw = record(cache, pending, lease);
                let source = own(&raw, &policy);
                let copied = SubscriptionRecord::copy_admitted(&source, &policy).unwrap();
                assert_same(&raw, &copied);
                assert!(!SharedOwned::ptr_eq(&source, &copied));
                assert_ne!(
                    source.owner.as_str().as_ptr(),
                    copied.owner.as_str().as_ptr()
                );
                assert!(copied.belongs_to(&policy));
                assert_eq!(
                    copied.requested_bytes(),
                    raw.copy_plan().unwrap().requested_bytes()
                );
                drop((source, copied));
                assert_eq!(root.available_bytes(), baseline);
            }
        }
    }
}

#[test]
fn copied_record_and_secret_outlive_their_source_and_intermediate_clones() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let source = own(&record(true, true, true), &policy);
    let copied = SubscriptionRecord::copy_admitted(&source, &policy).unwrap();
    let cache = SubscriptionRecord::cache_admitted(&source, &policy)
        .unwrap()
        .unwrap();
    assert_eq!(source.cache.as_ref().unwrap(), &*cache);
    assert_ne!(
        source.cache.as_ref().unwrap().expose().as_ptr(),
        cache.expose().as_ptr()
    );
    let retained_record = copied.clone();
    let retained_cache = cache.clone();
    let expected = copied.requested_bytes() + cache.requested_bytes();
    drop((source, copied, cache));
    assert_eq!(baseline - root.available_bytes(), expected);
    assert_eq!(retained_record.revision, 7);
    assert!(retained_cache.expose().starts_with("synthetic-cache"));
    drop(retained_record);
    assert_eq!(
        baseline - root.available_bytes(),
        retained_cache.requested_bytes()
    );
    drop(retained_cache);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn copied_secret_diagnostics_remain_redacted() {
    let (_, policy) = policy();
    let source = own(&record(true, false, false), &policy);
    let cache = SubscriptionRecord::cache_admitted(&source, &policy)
        .unwrap()
        .unwrap();
    let diagnostic = format!("{cache:?}");
    assert!(diagnostic.contains("redacted"));
    assert!(!diagnostic.contains("synthetic-cache"));
}

#[test]
fn copy_refuses_under_materialized_pressure_and_absent_cache_needs_no_capacity() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let source = own(&record(true, true, true), &policy);
    let empty = own(&record(false, false, false), &policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![3u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    for _ in 0..32 {
        assert!(matches!(
            SubscriptionRecord::copy_admitted(&source, &policy),
            Err(AdmissionRefusal::AtCapacity { .. })
        ));
        assert!(matches!(
            SubscriptionRecord::cache_admitted(&source, &policy),
            Err(AdmissionRefusal::AtCapacity { .. })
        ));
        assert!(SubscriptionRecord::cache_admitted(&empty, &policy)
            .unwrap()
            .is_none());
    }
    assert_eq!(pressure[pressure.len() - 1], 3);
    drop((pressure, source, empty));
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn foreign_root_and_sibling_attachment_refuse_even_an_absent_cache() {
    let (_, foreign) = policy();
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 2, plan(1), plan(1))
            .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let baseline = root.available_bytes();
    let first = parent.admit_attachment().unwrap();
    let second = parent.admit_attachment().unwrap();
    for cache in [true, false] {
        let source = own(&record(cache, false, false), first.policy());
        for wrong in [&foreign, second.policy()] {
            assert!(matches!(
                SubscriptionRecord::copy_admitted(&source, wrong),
                Err(AdmissionRefusal::InvalidPlan { .. })
            ));
            assert!(matches!(
                SubscriptionRecord::cache_admitted(&source, wrong),
                Err(AdmissionRefusal::InvalidPlan { .. })
            ));
        }
    }
    drop((first, second));
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn record_copy_refuses_attachment_downgrade_to_process_policy() {
    copy_refuses_attachment_downgrade(false);
}

#[test]
fn cache_copy_refuses_attachment_downgrade_even_without_cache() {
    copy_refuses_attachment_downgrade(true);
}

fn copy_refuses_attachment_downgrade(cache_only: bool) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 1, plan(1), plan(1))
            .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let attachment = parent.admit_attachment().unwrap();
    for cache in [true, false] {
        let source = own(&record(cache, false, false), attachment.policy());
        let before = root.available_bytes();
        let result = if cache_only {
            SubscriptionRecord::cache_admitted(&source, &parent).map(|_| ())
        } else {
            SubscriptionRecord::copy_admitted(&source, &parent).map(|_| ())
        };
        assert!(matches!(result, Err(AdmissionRefusal::InvalidPlan { .. })));
        assert_eq!(root.available_bytes(), before);
    }
}

#[test]
fn copied_secret_keeps_original_attachment_after_record_release() {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 1, plan(1), plan(1))
            .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let baseline = root.available_bytes();
    let attachment = parent.admit_attachment().unwrap();
    let source = own(&record(true, false, false), attachment.policy());
    let cache = SubscriptionRecord::cache_admitted(&source, attachment.policy())
        .unwrap()
        .unwrap();
    drop((source, attachment));
    assert!(parent.admit_attachment().is_err());
    drop(cache);
    drop(parent.admit_attachment().unwrap());
    assert_eq!(root.available_bytes(), baseline);
}

struct LegacyOnly;

#[async_trait::async_trait]
impl SubscriptionStore for LegacyOnly {
    async fn get(&self, _: &SubjectId) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
        panic!("admitted read invoked legacy store")
    }
    async fn insert(&self, _: SubscriptionRecord) -> Result<bool, SubscriptionError> {
        panic!("admitted refresh inserted a login record")
    }
    async fn replace(
        &self,
        _: &SubscriptionRecord,
        _: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError> {
        panic!("admitted replacement invoked legacy store")
    }
}

#[async_trait::async_trait]
impl NativeSubscriptionAuth for LegacyOnly {
    async fn start(&self) -> Result<NativeLogin, SubscriptionError> {
        panic!("admitted refresh started device login")
    }
    async fn refresh(&self, _: &SecretValue) -> Result<SecretValue, SubscriptionError> {
        panic!("admitted refresh invoked legacy native process")
    }
}

#[test]
fn unimplemented_ports_refuse_synchronously_without_legacy_fallback() {
    let (root, policy) = policy();
    let source = own(&record(true, false, false), &policy);
    let cache = SubscriptionRecord::cache_admitted(&source, &policy)
        .unwrap()
        .unwrap();
    let owner = policy
        .admit(
            IngressClass::Data,
            plan(sizing::shared_owner_bytes::<SubjectId>() + source.owner.as_str().len()),
        )
        .unwrap()
        .initialize(|| source.owner.clone());
    let available = root.available_bytes();
    let store: &dyn SubscriptionStore = &LegacyOnly;
    let native: &dyn NativeSubscriptionAuth = &LegacyOnly;
    assert!(matches!(
        store.get_admitted(&owner, &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    assert!(matches!(
        store.replace_admitted(&source, &source, &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    assert!(matches!(
        native.refresh_admitted(&cache, &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    assert_eq!(root.available_bytes(), available);
}
