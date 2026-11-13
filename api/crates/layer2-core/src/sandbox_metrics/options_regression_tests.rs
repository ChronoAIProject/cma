use super::*;

fn options(count: usize) -> Vec<FilterOption> {
    (0..count)
        .map(|i| FilterOption {
            value: format!("sandbox-{i}"),
            label: "Research".into(),
            disambiguator: None,
        })
        .collect()
}

#[test]
fn final_contexts_are_unique_when_an_owner_name_resembles_generated_context() {
    let at = |date: &str| {
        time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339)
            .unwrap()
            .unix_timestamp() as u64
    };
    let contexts = [
        OptionContext::sandbox("owner-a", Some("Alice"), Some(at("2026-11-01T12:30:00Z"))),
        OptionContext::sandbox("owner-b", Some("Alice"), Some(at("2026-11-02T12:30:00Z"))),
        OptionContext::sandbox(
            "owner-c",
            Some("Alice · Created 2026-11-01"),
            Some(at("2026-11-03T12:30:00Z")),
        ),
    ];
    let expected = [
        "Alice · Created 2026-11-01 12:30 UTC",
        "Alice · Created 2026-11-02",
        "Alice · Created 2026-11-01 · Created 2026-11-03",
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let original = options(3);
        let mut items = order.map(|i| original[i].clone());
        let reordered = order.map(|i| contexts[i].clone());
        disambiguate_options_with_context(&mut items, &reordered);
        for (item, index) in items.iter().zip(order) {
            assert_eq!(item.label, "Research");
            assert_eq!(item.disambiguator.as_deref(), Some(expected[index]));
            assert!(!item.disambiguator.as_deref().unwrap().contains("reference"));
        }
        assert_eq!(
            items
                .iter()
                .map(|item| (&item.label, &item.disambiguator))
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
    }
}

#[test]
fn missing_detail_waits_for_other_human_context_before_using_a_reference() {
    let mut items = options(2);
    let contexts = [
        OptionContext::sandbox("alice", Some("Alice"), None),
        OptionContext::sandbox("alice", Some("Alice"), Some(1000)),
    ];
    disambiguate_options_with_context(&mut items, &contexts);
    assert_eq!(items[0].disambiguator.as_deref(), Some("Alice"));
    assert_eq!(
        items[1].disambiguator.as_deref(),
        Some("Alice · Created 1970-01-01")
    );
}

#[test]
fn a_literal_reference_looking_hint_cannot_collide_with_the_last_resort() {
    use sha2::{Digest, Sha256};
    let mut items = options(3);
    let digest = format!("{:x}", Sha256::digest(items[0].value.as_bytes()));
    let literal = format!("Alice · reference {}", &digest[..12]);
    disambiguate_options(
        &mut items,
        &[
            Some("Alice".into()),
            Some("Alice".into()),
            Some(literal.clone()),
        ],
    );
    assert_eq!(items[2].disambiguator, Some(literal));
    assert_eq!(
        items[0].disambiguator,
        Some(format!("Alice · reference {digest}"))
    );
    assert_eq!(
        items
            .iter()
            .map(|item| &item.disambiguator)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
}

#[test]
fn owner_precision_defaults_to_dates_and_requires_directory_capability() {
    use crate::{Authorization, Capability};
    for auth in [
        Authorization::default(),
        Authorization::new([Capability::ReadAllSandboxes]),
    ] {
        let mut items = options(2);
        let contexts = ["2026-11-01T12:30:00.001Z", "2026-11-01T12:30:00.002Z"].map(|at| {
            OptionContext::owner_with_access(
                Some(at),
                OwnerContextAccess::from_authorization(&auth),
            )
        });
        disambiguate_options_with_context(&mut items, &contexts);
        for item in &items {
            let detail = item.disambiguator.as_deref().unwrap();
            assert!(detail.starts_with("Joined 2026-11-01 · reference "));
            assert!(!detail.contains("12:30") && !detail.contains("UTC"));
        }
    }
    let mut items = options(2);
    let contexts = ["2026-11-01T12:30:00.001Z", "2026-11-01T12:30:00.002Z"]
        .map(|at| OptionContext::owner(Some(at)));
    disambiguate_options_with_context(&mut items, &contexts);
    assert!(items.iter().all(|item| item
        .disambiguator
        .as_deref()
        .unwrap()
        .starts_with("Joined 2026-11-01 · reference ")));
    let auth = Authorization::new([Capability::ReadAllSandboxes, Capability::ReadUserDirectory]);
    let contexts = ["2026-11-01T12:30:00.001Z", "2026-11-01T12:30:00.002Z"].map(|at| {
        OptionContext::owner_with_access(Some(at), OwnerContextAccess::from_authorization(&auth))
    });
    disambiguate_options_with_context(&mut items, &contexts);
    assert_eq!(
        items[0].disambiguator.as_deref(),
        Some("Joined 2026-11-01 12:30:00.001 UTC")
    );
    assert_eq!(
        items[1].disambiguator.as_deref(),
        Some("Joined 2026-11-01 12:30:00.002 UTC")
    );
}
