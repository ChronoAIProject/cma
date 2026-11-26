use super::*;
use layer3_adapter::ingress_budget::{ClassBounds, IngressBudget};

thread_local! {
    pub(super) static SERIALIZER_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic native request fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(32 * 1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn scope(policy: &IngressOwnershipPolicy) -> IngressWorkScope {
    IngressWorkScope::new(
        policy
            .admit(IngressClass::Data, plan(sizing::shared_owner_bytes::<()>()))
            .unwrap(),
    )
}

fn original(request: NativeRefreshRequest, version: &str) -> Vec<u8> {
    use serde_json::json;
    let value = match request {
        NativeRefreshRequest::Initialize { id } => json!({
            "id": id, "method": "initialize", "params": {
                "clientInfo": {"name": "cma_subscription_connection", "version": version},
                "capabilities": {"experimentalApi": false}
            }
        }),
        NativeRefreshRequest::Initialized => json!({"method": "initialized"}),
        NativeRefreshRequest::ReadAccount { id } => {
            json!({"id": id, "method": "account/read", "params": {"refreshToken": true}})
        }
    };
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

fn requests(id: u64) -> [NativeRefreshRequest; 3] {
    [
        NativeRefreshRequest::Initialize { id },
        NativeRefreshRequest::Initialized,
        NativeRefreshRequest::ReadAccount { id },
    ]
}

#[test]
fn all_messages_match_exact_legacy_bytes_at_id_boundaries() {
    let (root, policy) = policy();
    let source = scope(&policy);
    let baseline = root.available_bytes();
    for id in [0, 1, u64::MAX] {
        for request in requests(id) {
            let expected = original(request, env!("CARGO_PKG_VERSION"));
            let result = request.encode_owned(&source, &policy).unwrap();
            assert_eq!(result.expose().as_bytes(), expected);
            assert_eq!(expected.iter().filter(|byte| **byte == b'\n').count(), 1);
            assert!(result.shares_custody_domain(&policy));
            let capacity = request.maximum_bytes(env!("CARGO_PKG_VERSION")).unwrap();
            assert_eq!(
                result.requested_bytes(),
                output_plan(capacity).unwrap().requested_bytes()
            );
            assert_eq!(baseline - root.available_bytes(), result.requested_bytes());
            drop(result);
            assert_eq!(root.available_bytes(), baseline);
        }
    }
}

#[test]
fn maximum_bound_includes_exact_version_escapes_and_final_lf() {
    let (_, policy) = policy();
    let source = scope(&policy);
    let controls: String = (0..=0x1f).map(char::from).collect();
    for version in [
        "",
        "plain",
        "quote\"slash\\",
        "合成🦀/\u{7f}",
        controls.as_str(),
    ] {
        let request = NativeRefreshRequest::Initialize { id: u64::MAX };
        let capacity = request.maximum_bytes(version).unwrap();
        let expected = original(request, version);
        assert_eq!(capacity, expected.len());
        let pending = RequestBuffer::admit(&source, &policy, capacity).unwrap();
        let pointer = pending.bytes.as_ptr();
        let result = pending.encode(request, version).unwrap();
        assert_eq!(result.expose().as_bytes(), expected);
        assert_eq!(result.expose().as_ptr(), pointer);
    }
}

#[test]
fn escape_bound_matches_serializer_for_every_ascii_byte() {
    for byte in 0..=127 {
        let value = char::from(byte).to_string();
        assert_eq!(
            escaped_bytes(&value).unwrap() + 2,
            serde_json::to_vec(&value).unwrap().len()
        );
    }
}

#[test]
fn exact_capacity_and_one_byte_short_include_newline() {
    let (root, policy) = policy();
    let source = scope(&policy);
    let baseline = root.available_bytes();
    for request in requests(u64::MAX) {
        let version = env!("CARGO_PKG_VERSION");
        let capacity = request.maximum_bytes(version).unwrap();
        assert_eq!(capacity, original(request, version).len());
        let result = RequestBuffer::admit(&source, &policy, capacity)
            .unwrap()
            .encode(request, version)
            .unwrap();
        drop(result);
        for short in [1, 2] {
            assert_eq!(
                RequestBuffer::admit(&source, &policy, capacity - short)
                    .unwrap()
                    .encode(request, version)
                    .unwrap_err(),
                NativeFrameError::Unavailable
            );
            assert_eq!(root.available_bytes(), baseline);
        }
    }
}

#[test]
fn writer_refuses_before_copy_and_retains_initialized_suffix() {
    let mut bytes = [0; 4];
    let mut writer = BoundedWriter {
        bytes: &mut bytes,
        used: 0,
    };
    writer.write_all(b"ab").unwrap();
    assert_eq!(
        writer.write(b"cde").unwrap_err().kind(),
        io::ErrorKind::WriteZero
    );
    assert_eq!(writer.used, 2);
    writer.write_all(b"cd").unwrap();
    writer.flush().unwrap();
    assert_eq!(bytes, *b"abcd");
}

#[test]
fn real_serializer_error_uses_static_unavailable() {
    let (root, policy) = policy();
    let source = scope(&policy);
    let baseline = root.available_bytes();
    let result = RequestBuffer::admit(&source, &policy, 1)
        .unwrap()
        .encode(NativeRefreshRequest::Initialize { id: 1 }, "synthetic");
    assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn overflow_is_refused_before_storage_construction() {
    assert!(checked_sum(&[usize::MAX, 1]).is_err());
    assert!(request_plan(usize::MAX).is_err());
    assert!(output_plan(usize::MAX).is_err());
}

#[test]
fn foreign_roots_refuse_request_admission() {
    let (_, first) = policy();
    let (_, second) = policy();
    let source = scope(&first);
    let entries = SERIALIZER_ENTRIES.get();
    for request in requests(u64::MAX) {
        assert!(matches!(
            request.encode_owned(&source, &second),
            Err(NativeFrameError::Refused(
                AdmissionRefusal::InvalidPlan { .. }
            ))
        ));
    }
    assert_eq!(SERIALIZER_ENTRIES.get(), entries);
}

fn attached_policy(slots: usize) -> IngressOwnershipPolicy {
    let root = IngressBudget::new(64 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        slots,
        plan(4096),
        plan(16 * 1024 * 1024),
    )
    .unwrap();
    IngressOwnershipPolicy::new(root, bounds).unwrap()
}

#[test]
fn process_attachment_and_sibling_domains_are_distinct() {
    let policy = attached_policy(2);
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let process = scope(&policy);
    let attached = scope(first.policy());
    let entries = SERIALIZER_ENTRIES.get();
    for (source, destination) in [
        (&process, first.policy()),
        (&attached, &policy),
        (&attached, second.policy()),
    ] {
        for request in requests(1) {
            assert!(matches!(
                request.encode_owned(source, destination),
                Err(NativeFrameError::Refused(
                    AdmissionRefusal::InvalidPlan { .. }
                ))
            ));
        }
    }
    assert_eq!(SERIALIZER_ENTRIES.get(), entries);
    for request in requests(1) {
        let result = request.encode_owned(&attached, first.policy()).unwrap();
        assert!(result.shares_custody_domain(first.policy()));
    }
}

#[test]
fn materialized_pressure_refuses_before_encoding() {
    let (root, policy) = policy();
    let source = scope(&policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![1u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    let baseline = root.available_bytes();
    let entries = SERIALIZER_ENTRIES.get();
    for request in requests(u64::MAX) {
        assert!(matches!(
            request.encode_owned(&source, &policy),
            Err(NativeFrameError::Refused(_))
        ));
        assert_eq!(root.available_bytes(), baseline);
    }
    assert_eq!(SERIALIZER_ENTRIES.get(), entries);
    drop(pressure);
}

#[cfg(test)]
#[path = "native_codex_ingress_request_lifetime_tests.rs"]
mod lifetime;
