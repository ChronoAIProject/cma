use super::*;
use crate::connections::subscription_tokens::tests::cache;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use layer3_adapter::ingress_budget::{ClassBounds, IngressBudget};
use std::time::{Duration, SystemTime};

fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("synthetic cache fixture", &[bytes]).unwrap()
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(16 * 1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

fn own(cache: &SecretValue, policy: &IngressOwnershipPolicy) -> SharedOwned<SecretValue> {
    policy
        .admit(
            IngressClass::Data,
            plan(sizing::shared_owner_bytes::<SecretValue>() + cache.expose().len()),
        )
        .unwrap()
        .initialize(|| cache.clone())
}

fn assert_parity(cache: &SecretValue) {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let input = own(cache, &policy);
    let before = root.available_bytes();
    let original = SubscriptionTokens::from_native_cache(cache);
    let admitted = SubscriptionTokens::from_native_cache_admitted(&input, &policy);
    match (original, admitted) {
        (Ok(original), Ok(admitted)) => {
            assert_eq!(original.access_token, admitted.access_token);
            assert_eq!(original.account_id, admitted.account_id);
            assert_eq!(original.plan_type, admitted.plan_type);
            assert_eq!(original.expires_at, admitted.expires_at);
            assert!(admitted.belongs_to(&policy));
            assert_eq!(before - root.available_bytes(), admitted.requested_bytes());
            assert!(
                admitted.requested_bytes()
                    < decode_plan(cache.expose().len()).unwrap().requested_bytes()
            );
        }
        (Err(original), Err(CacheDecodeError::Account(admitted))) => {
            assert_eq!(original, admitted);
        }
        outcome => panic!("decoder outcomes differ: {outcome:?}"),
    }
    assert_eq!(root.available_bytes(), before);
    drop(input);
    assert_eq!(root.available_bytes(), baseline);
}

fn replace(cache: &SecretValue, from: &str, to: &str) -> SecretValue {
    assert!(cache.expose().contains(from));
    SecretValue::new(cache.expose().replace(from, to))
}

fn with_claims(claims: &str) -> SecretValue {
    SecretValue::new(
        serde_json::json!({"tokens": {
            "access_token": format!("fixture.{}.fixture", URL_SAFE_NO_PAD.encode(claims)),
            "refresh_token": "synthetic-refresh", "id_token": "synthetic-id",
            "account_id": "one"
        }})
        .to_string(),
    )
}

#[test]
fn admitted_cache_preserves_credentials_expiry_and_redaction() {
    let raw = cache("one", "one", 1000);
    assert_parity(&raw);
    let (_, policy) = policy();
    let tokens =
        SubscriptionTokens::from_native_cache_admitted(&own(&raw, &policy), &policy).unwrap();
    assert!(tokens.usable_at(SystemTime::UNIX_EPOCH + Duration::from_secs(879)));
    assert!(!tokens.usable_at(SystemTime::UNIX_EPOCH + Duration::from_secs(880)));
    assert!(!format!("{tokens:?}").contains(tokens.access_token.expose()));
}

#[test]
fn admitted_cache_preserves_domain_rejections_and_releases_scratch() {
    for raw in [
        cache("one", "two", 1000),
        cache("", "", 1000),
        cache(&"x".repeat(257), &"x".repeat(257), 1000),
        cache("a\nb", "a\nb", 1000),
        cache("one", "one", u64::MAX),
        replace(&cache("one", "one", 1000), "fixture-refresh-secret", "  "),
        replace(&cache("one", "one", 1000), "fixture-id-secret", ""),
    ] {
        assert!(SubscriptionTokens::from_native_cache(&raw).is_err());
        assert_parity(&raw);
    }
}

#[test]
fn admitted_cache_preserves_jwt_segment_and_base64_rules() {
    let raw = cache("one", "one", 1000);
    // Empty header/signature were accepted by the original decoder.
    assert_parity(&replace(&raw, "fixture.", "."));
    assert_parity(&replace(&raw, ".fixture", "."));
    for replacement in ["fixture", "fixture.", "fixture.a.b.c", "fixture.!.fixture"] {
        let mut value: serde_json::Value = serde_json::from_str(raw.expose()).unwrap();
        value["tokens"]["access_token"] = replacement.into();
        let invalid = SecretValue::new(value.to_string());
        assert!(SubscriptionTokens::from_native_cache(&invalid).is_err());
        assert_parity(&invalid);
    }
}

#[test]
fn admitted_cache_handles_partial_json_wrong_types_and_diagnostics() {
    let raw = cache("one", "one", 1000);
    for cut in [0, 1, raw.expose().len() / 2, raw.expose().len() - 1] {
        assert_parity(&SecretValue::new(raw.expose()[..cut].to_owned()));
    }
    for invalid in [
        "{}",
        "[]",
        "null",
        r#"{"tokens":null}"#,
        r#"{"tokens":{"access_token":[]}}"#,
    ] {
        assert_parity(&SecretValue::new(invalid.into()));
    }
    // Wrong scalar types force serde's heap-bearing diagnostic path.
    let wrong_type = serde_json::json!({"tokens": "\u{7f}".repeat(16000)}).to_string();
    assert_parity(&SecretValue::new(wrong_type));
    let wrong_claim = serde_json::json!({"exp": "\u{7f}".repeat(16000)}).to_string();
    assert_parity(&with_claims(&wrong_claim));
    assert_parity(&with_claims(r#"{"exp":-1}"#));
    assert_parity(&with_claims("{"));
}

#[test]
fn admitted_cache_preserves_numeric_overflow_and_float_rejections() {
    for number in [
        "18446744073709551616".to_owned(),
        "1.00000000000000011102230246251565404236316680908203125".to_owned(),
        "2.2250738585072011e-308".to_owned(),
        "4.9406564584124654e-324".to_owned(),
        "1e2147483648".to_owned(),
        "1e-2147483648".to_owned(),
        format!("0.{}1", "0".repeat(60000)),
        format!("1.{}e-20", "1".repeat(60000)),
    ] {
        for raw in [
            SecretValue::new(format!("{{\"tokens\":{number}}}")),
            with_claims(&format!("{{\"exp\":{number}}}")),
        ] {
            assert!(SubscriptionTokens::from_native_cache(&raw).is_err());
            assert_parity(&raw);
        }
    }
}

#[test]
fn admitted_cache_handles_escaped_unicode_and_unknown_nested_values() {
    assert_parity(&cache("账户🦀", "账户🦀", 1000));
    let raw = cache("one", "one", 1000);
    assert_parity(&replace(&raw, "one", r"o\u006ee"));
    let nested = format!("{}0{}", "[".repeat(12000), "]".repeat(12000));
    let unknown = format!("{{\"ignored\":{nested},{}", &raw.expose()[1..]);
    assert_parity(&SecretValue::new(unknown));
    let claims = r#"{"exp":1000,"https://api.openai.com/auth":{"chatgpt_account_id":"one","chatgpt_plan_type":"\u00e9\uD83E\uDD80"}}"#;
    assert_parity(&with_claims(claims));
    assert_parity(&with_claims(&claims.replace(r"\uDD80", r"\u0000")));
}

#[test]
fn admitted_cache_keeps_the_exact_input_limit() {
    let raw = cache("one", "one", 1000);
    let mut maximum = raw.expose().to_owned();
    maximum.extend(std::iter::repeat_n(' ', MAX_CACHE_BYTES - maximum.len()));
    let maximum = SecretValue::new(maximum);
    assert!(SubscriptionTokens::from_native_cache(&maximum).is_ok());
    assert_parity(&maximum);
    let oversized = SecretValue::new(format!("{} ", maximum.expose()));
    assert_parity(&oversized);
    let (root, policy) = policy();
    let input = own(&oversized, &policy);
    let occupied = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let before = root.available_bytes();
    assert_eq!(
        SubscriptionTokens::from_native_cache_admitted(&input, &policy).unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::InvalidAccount)
    );
    assert_eq!(root.available_bytes(), before);
    drop(occupied);
}

#[test]
fn admitted_cache_refuses_data_pressure_before_decoding() {
    let (root, policy) = policy();
    let malformed = own(&SecretValue::new("{".into()), &policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let occupied = policy.admit(IngressClass::Data, plan(available)).unwrap();
    let before = root.available_bytes();
    assert!(matches!(
        SubscriptionTokens::from_native_cache_admitted(&malformed, &policy),
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::AtCapacity { .. }
        ))
    ));
    assert_eq!(root.available_bytes(), before);
    drop(occupied);
    assert_eq!(
        SubscriptionTokens::from_native_cache_admitted(&malformed, &policy).unwrap_err(),
        CacheDecodeError::Account(SubscriptionError::InvalidAccount)
    );
}

#[test]
fn admitted_cache_refuses_foreign_root_before_parsing() {
    let (root, local) = policy();
    let (foreign_root, foreign) = policy();
    let input = own(&SecretValue::new("{".into()), &local);
    let before = (root.available_bytes(), foreign_root.available_bytes());
    assert_eq!(
        SubscriptionTokens::from_native_cache_admitted(&input, &foreign).unwrap_err(),
        CacheDecodeError::Refused(AdmissionRefusal::InvalidPlan {
            phase: "subscription cache ownership"
        })
    );
    assert_eq!(
        (root.available_bytes(), foreign_root.available_bytes()),
        before
    );
}

#[test]
fn admitted_cache_result_survives_input_and_source_drop() {
    let (root, policy) = policy();
    let baseline = root.available_bytes();
    let input = own(&cache("one", "one", 1000), &policy);
    let result = SubscriptionTokens::from_native_cache_admitted(&input, &policy).unwrap();
    let retained = result.requested_bytes();
    let clone = result.clone();
    drop((result, input, policy));
    assert_eq!(root.available_bytes(), baseline - retained);
    assert_eq!(clone.account_id, "one");
    drop(clone);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn admitted_cache_enforces_attachment_identity_and_final_slot_lifetime() {
    let root = IngressBudget::new(512 * 1024 * 1024);
    let bounds = IngressOwnershipPolicy::conversation_bounds(
        root.capacity_bytes(),
        2,
        plan(4096),
        plan(96 * 1024 * 1024),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let baseline = root.available_bytes();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let input = own(&cache("one", "one", 1000), first.policy());
    let unbound = own(&cache("one", "one", 1000), &policy);
    for wrong in [&input, &unbound] {
        assert!(matches!(
            SubscriptionTokens::from_native_cache_admitted(wrong, second.policy()),
            Err(CacheDecodeError::Refused(
                AdmissionRefusal::InvalidPlan { .. }
            ))
        ));
    }
    let result = SubscriptionTokens::from_native_cache_admitted(&input, first.policy()).unwrap();
    assert!(result.belongs_to(first.policy()));
    assert!(!result.belongs_to(second.policy()));
    let clone = result.clone();
    drop((first, input, unbound, result));
    assert!(policy.admit_attachment().is_err());
    drop(clone);
    drop(policy.admit_attachment().unwrap());
    drop(second);
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn admitted_cache_refuses_attachment_downgrade_before_parsing_or_admission() {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 1, plan(1), plan(1))
            .unwrap();
    let parent = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let attachment = parent.admit_attachment().unwrap();
    for raw in [
        cache("one", "one", 1000),
        SecretValue::new("{".into()),
        SecretValue::new(" ".repeat(MAX_CACHE_BYTES + 1)),
    ] {
        let input = own(&raw, attachment.policy());
        let before = root.available_bytes();
        assert_eq!(
            SubscriptionTokens::from_native_cache_admitted(&input, &parent).unwrap_err(),
            CacheDecodeError::Refused(AdmissionRefusal::InvalidPlan {
                phase: "subscription cache ownership"
            })
        );
        assert_eq!(root.available_bytes(), before);
    }
}

#[test]
fn admitted_cache_layout_refuses_overflow() {
    for bytes in [usize::MAX, usize::MAX / 10, u32::MAX as usize] {
        assert!(matches!(
            decode_plan(bytes),
            Err(AdmissionRefusal::InvalidPlan { .. })
        ));
    }
    assert!(decode_plan(MAX_CACHE_BYTES).is_ok());
}
