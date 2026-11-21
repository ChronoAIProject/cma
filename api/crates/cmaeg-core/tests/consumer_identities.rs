//! The conversation and idempotency identities of non-default consumers.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    ids::{
        compact_partition_key, ChannelId, ConsumerId, ConversationId, EventId, IdempotencyKey,
        MAX_SEALED_PARTITION_KEY_BYTES,
    },
    seal::MasterKeyRing,
};

fn ring(spec: &[(u8, u8)]) -> MasterKeyRing {
    let entries = spec
        .iter()
        .map(|(kid, fill)| format!("{kid}:{}", STANDARD.encode([*fill; 32])))
        .collect::<Vec<_>>();
    MasterKeyRing::parse(&entries.join(",")).unwrap()
}

fn channel(name: &str) -> ChannelId {
    ChannelId::new(format!("cmaeg1.ch.{name}")).unwrap()
}

fn consumer(id: &str) -> ConsumerId {
    ConsumerId::parse(id).unwrap()
}

#[test]
fn consumer_conversations_are_disjoint_from_default_conversations() {
    let keys = ring(&[(1, 1)]);
    let reviewer = consumer("csm_00000000000a");
    let id = ConversationId::derive_for_consumer(&keys, &channel("a"), 1, &reviewer, "42");
    assert!(id.is_wellformed());
    assert_ne!(id, ConversationId::derive(&keys, &channel("a"), 1, "42"));
    // The consumer's id joined into the key cannot reach the default either.
    assert_ne!(
        id,
        ConversationId::derive(&keys, &channel("a"), 1, "csm_00000000000a\u{1f}42")
    );
}

#[test]
fn consumer_conversations_differ_per_consumer_channel_version_and_key() {
    let keys = ring(&[(1, 1)]);
    let reviewer = consumer("csm_00000000000a");
    let base = ConversationId::derive_for_consumer(&keys, &channel("a"), 1, &reviewer, "42");
    assert_eq!(
        base,
        ConversationId::derive_for_consumer(&keys, &channel("a"), 1, &reviewer, "42")
    );
    for other in [
        ConversationId::derive_for_consumer(
            &keys,
            &channel("a"),
            1,
            &consumer("csm_00000000000b"),
            "42",
        ),
        ConversationId::derive_for_consumer(&keys, &channel("b"), 1, &reviewer, "42"),
        ConversationId::derive_for_consumer(&keys, &channel("a"), 2, &reviewer, "42"),
        ConversationId::derive_for_consumer(&keys, &channel("a"), 1, &reviewer, "43"),
    ] {
        assert_ne!(base, other);
    }
}

#[test]
fn consumer_conversations_survive_a_key_rotation_through_the_retained_kid() {
    let before = ring(&[(1, 1)]);
    let reviewer = consumer("csm_00000000000a");
    let id = ConversationId::derive_for_consumer(&before, &channel("a"), 1, &reviewer, "42");
    let rotated = ring(&[(2, 2), (1, 1)]);
    assert_eq!(rotated.derivation_kid(), 2);
    assert_ne!(
        id,
        ConversationId::derive_for_consumer(&rotated, &channel("a"), 1, &reviewer, "42")
    );
    assert_eq!(
        id,
        ConversationId::derive_for_consumer_with_kid(
            &rotated,
            1,
            &channel("a"),
            1,
            &reviewer,
            "42"
        )
    );
}

#[test]
fn a_long_partition_key_and_its_compact_form_give_the_same_consumer_conversation() {
    let keys = ring(&[(1, 1)]);
    let reviewer = consumer("csm_00000000000a");
    let raw = format!("urn:subject:{}", "s".repeat(588));
    assert_eq!(raw.len(), 600);
    let compact = compact_partition_key(&raw);
    assert_eq!(compact.len(), "sha256:".len() + 64);
    assert!(compact.starts_with("sha256:"));
    assert_eq!(
        compact,
        format!("sha256:{}", ring_digest_hex(raw.as_bytes()))
    );
    // Ingress compacts the raw subject; a hand-off holds only the sealed form.
    assert_eq!(
        ConversationId::derive_for_consumer(&keys, &channel("a"), 1, &reviewer, &compact),
        ConversationId::derive_for_consumer(
            &keys,
            &channel("a"),
            1,
            &reviewer,
            &compact_partition_key(&compact)
        )
    );
}

#[test]
fn partition_keys_up_to_the_sealed_bound_stay_raw() {
    let at_bound = "k".repeat(MAX_SEALED_PARTITION_KEY_BYTES);
    assert_eq!(compact_partition_key(&at_bound), at_bound);
    assert_eq!(compact_partition_key("42\u{1f}7"), "42\u{1f}7");
    let over = "k".repeat(MAX_SEALED_PARTITION_KEY_BYTES + 1);
    assert!(compact_partition_key(&over).starts_with("sha256:"));
}

#[test]
fn compact_partition_keys_use_the_lowercase_sha256_hex_of_the_raw_key() {
    // A known SHA-256 vector, computed independently of this crate.
    assert_eq!(
        compact_partition_key(&"a".repeat(1000)),
        "sha256:41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "default consumer keeps ConversationId::derive")]
fn the_default_consumer_never_derives_a_consumer_conversation() {
    let keys = ring(&[(1, 1)]);
    let _ = ConversationId::derive_for_consumer(
        &keys,
        &channel("a"),
        1,
        &ConsumerId::default_consumer(),
        "42",
    );
}

#[test]
fn consumer_idempotency_keys_are_bounded_valid_and_collision_free() {
    let reviewer = consumer("csm_00000000000a");
    let event = EventId::new("telegram:123456:1001").unwrap();
    let key = IdempotencyKey::for_consumer(&event, &reviewer);
    assert_eq!(key.as_str().len(), 61);
    assert!(key.as_str().starts_with("evc_csm_00000000000a_"));
    assert_eq!(IdempotencyKey::new(key.as_str()).unwrap(), key);
    assert_eq!(key, IdempotencyKey::for_consumer(&event, &reviewer));
    assert_ne!(
        key,
        IdempotencyKey::for_consumer(&event, &consumer("csm_00000000000b"))
    );
    assert_ne!(key, IdempotencyKey::from_event(&event));

    // Two ids that the 200-byte evt_ form truncates to the same key.
    let first = EventId::new(format!("{}1", "e".repeat(200))).unwrap();
    let second = EventId::new(format!("{}2", "e".repeat(200))).unwrap();
    assert_eq!(
        IdempotencyKey::from_event(&first),
        IdempotencyKey::from_event(&second)
    );
    assert_ne!(
        IdempotencyKey::for_consumer(&first, &reviewer),
        IdempotencyKey::for_consumer(&second, &reviewer)
    );
    assert_eq!(
        IdempotencyKey::for_consumer(&first, &reviewer)
            .as_str()
            .len(),
        61
    );
}

fn ring_digest_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
